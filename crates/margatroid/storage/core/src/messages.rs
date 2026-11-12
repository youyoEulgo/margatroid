use super::*;
use rusqlite::{OptionalExtension, Row};
use serde_json::{Map, Value};

pub(crate) const COLUMNS: &str = "sequence,role,content,data,tool_call,payload";

#[derive(Debug)]
pub(crate) struct MessageRow {
    pub sequence: Option<i64>,
    pub role: String,
    pub content: Option<String>,
    pub data: Option<String>,
    pub tool_call: Option<String>,
    pub payload: String,
}

fn encode(value: &Value) -> Result<String, StorageError> {
    serde_json::to_string(value).map_err(write_err)
}
fn take_json(object: &mut Map<String, Value>, key: &str) -> Result<Option<String>, StorageError> {
    object.remove(key).as_ref().map(encode).transpose()
}

pub(crate) fn split(message: &Value) -> Result<MessageRow, StorageError> {
    let mut object = message
        .as_object()
        .cloned()
        .ok_or_else(|| err(K::InvalidRecord, "message must be an object"))?;
    if object.contains_key("replay") || object.contains_key("images") {
        return Err(err(
            K::InvalidRecord,
            "message contains unsupported protocol fields",
        ));
    }
    let sequence = match object.remove("sequence") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|v| *v > 0 && *v <= i64::MAX as u64)
                .ok_or_else(|| err(K::InvalidRecord, "invalid message history sequence"))?
                as i64,
        ),
    };
    let role = object
        .remove("type")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| err(K::InvalidRecord, "message type is required"))?;
    let content = match role.as_str() {
        "system" | "user" | "tool_call" => Some(
            object
                .remove("content")
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| err(K::InvalidRecord, "message content must be text"))?,
        ),
        "assistant" => {
            let mut text = Map::new();
            for key in ["reasoning", "content"] {
                if let Some(value) = object.remove(key) {
                    if !value.is_null() && !value.is_string() {
                        return Err(err(
                            K::InvalidRecord,
                            "assistant text must be string or null",
                        ));
                    }
                    text.insert(key.into(), value);
                }
            }
            Some(encode(&Value::Object(text))?)
        }
        "inject" => {
            let nested = object
                .remove("messages")
                .filter(Value::is_array)
                .ok_or_else(|| err(K::InvalidRecord, "inject messages must be an array"))?;
            for message in nested.as_array().unwrap() {
                split(message)?;
            }
            Some(encode(&nested)?)
        }
        "error" => Some(
            object
                .remove("message")
                .and_then(|v| v.as_str().map(str::to_owned))
                .ok_or_else(|| err(K::InvalidRecord, "error message must be text"))?,
        ),
        _ => return Err(err(K::InvalidRecord, "unsupported message role")),
    };
    let data = take_json(&mut object, "data")?;
    let tool_call = take_json(&mut object, "tool_calls")?;
    Ok(MessageRow {
        sequence,
        role,
        content,
        data,
        tool_call,
        payload: encode(&Value::Object(object))?,
    })
}

pub(crate) fn from_row(row: &Row<'_>, offset: usize) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        sequence: row.get(offset)?,
        role: row.get(offset + 1)?,
        content: row.get(offset + 2)?,
        data: row.get(offset + 3)?,
        tool_call: row.get(offset + 4)?,
        payload: row.get(offset + 5)?,
    })
}
pub(crate) fn join(row: MessageRow, with_sequence: bool) -> Result<Value, StorageError> {
    let mut object: Map<String, Value> = serde_json::from_str(&row.payload).map_err(schema_err)?;
    let mut reserved = vec!["type", "sequence", "tool_calls", "data", "replay", "images"];
    match row.role.as_str() {
        "system" | "user" | "tool_call" => reserved.push("content"),
        "assistant" => reserved.extend(["content", "reasoning"]),
        "inject" => reserved.push("messages"),
        "error" => reserved.push("message"),
        _ => return Err(schema_err("invalid message role")),
    }
    if reserved.iter().any(|k| object.contains_key(*k)) {
        return Err(schema_err("duplicate message payload fields"));
    }
    let content = row
        .content
        .ok_or_else(|| schema_err("missing message content"))?;
    match row.role.as_str() {
        "system" | "user" | "tool_call" => {
            object.insert("content".into(), Value::String(content));
        }
        "assistant" => {
            let text: Map<String, Value> = serde_json::from_str(&content).map_err(schema_err)?;
            if text.keys().any(|k| k != "reasoning" && k != "content") {
                return Err(schema_err("invalid assistant content"));
            }
            object.extend(text);
        }
        "inject" => {
            object.insert(
                "messages".into(),
                serde_json::from_str(&content).map_err(schema_err)?,
            );
        }
        "error" => {
            object.insert("message".into(), Value::String(content));
        }
        _ => return Err(schema_err("invalid message role")),
    }
    if let Some(data) = row.data {
        object.insert(
            "data".into(),
            serde_json::from_str(&data).map_err(schema_err)?,
        );
    }
    if let Some(calls) = row.tool_call {
        object.insert(
            "tool_calls".into(),
            serde_json::from_str(&calls).map_err(schema_err)?,
        );
    }
    object.insert("type".into(), Value::String(row.role));
    if with_sequence {
        if let Some(sequence) = row.sequence {
            object.insert("sequence".into(), Value::from(sequence));
        }
    }
    let value = Value::Object(object);
    split(&value).map_err(schema_err)?;
    Ok(value)
}

pub(crate) fn validate_sequence(db: &Connection, message: &Value) -> Result<(), StorageError> {
    let row = split(message)?;
    let Some(sequence) = row.sequence else {
        return Ok(());
    };
    let stored = db
        .query_row(
            &format!("SELECT {COLUMNS} FROM agent_messages WHERE sequence=?1"),
            [sequence],
            |r| from_row(r, 0),
        )
        .optional()
        .map_err(read_err)?
        .ok_or_else(|| err(K::InvalidRecord, "message history sequence does not exist"))?;
    if join(stored, false)? != join(row, false)? {
        return Err(err(
            K::InvalidRecord,
            "message history sequence content mismatch",
        ));
    }
    Ok(())
}
pub(crate) fn next_sequence(db: &Connection) -> Result<i64, StorageError> {
    let last:i64=db.query_row("SELECT max(sequence) FROM (SELECT coalesce(max(sequence),0) AS sequence FROM agent_messages UNION ALL SELECT coalesce(max(sequence),0) FROM mcl_records)",[],|r|r.get(0)).map_err(read_err)?;
    last.checked_add(1)
        .ok_or_else(|| err(K::InvalidRecord, "history sequence exhausted"))
}
pub(crate) fn append(
    db: &Connection,
    message: &Value,
    turn: Option<&str>,
    timestamp: i64,
) -> Result<Value, StorageError> {
    validate_sequence(db, message)?;
    let mut row = split(message)?;
    if row.sequence.is_none() {
        row.sequence = Some(next_sequence(db)?);
        db.execute("INSERT INTO agent_messages(sequence,turn,role,content,data,tool_call,payload,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![row.sequence,turn,row.role,row.content,row.data,row.tool_call,row.payload,timestamp]).map_err(write_err)?;
    }
    join(row, true)
}

pub fn append_message_history(
    storage: &AgentStorage,
    messages: &[Value],
    turn: Option<&str>,
) -> Result<Vec<Value>, StorageError> {
    let state = &storage.database.shared;
    let db = state
        .connection
        .lock()
        .map_err(|_| err(K::Closed, "database lock poisoned"))?;
    if state.retired.load(Ordering::Acquire) {
        return Err(err(K::CleanupFailed, "database connection retired"));
    }
    let timestamp: i64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(write_err)?
        .as_millis()
        .try_into()
        .map_err(write_err)?;
    db.execute_batch("BEGIN IMMEDIATE").map_err(write_err)?;
    let result: Result<Vec<Value>, StorageError> = messages
        .iter()
        .map(|m| append(&db, m, turn, timestamp))
        .collect();
    let result = result.and_then(|value| Ok((value, pages::record_watermarks(&db)?)));
    let (value, committed) = finish_transaction(&db, &state.retired, result)?;
    publish_record_changes(state, committed);
    Ok(value)
}
