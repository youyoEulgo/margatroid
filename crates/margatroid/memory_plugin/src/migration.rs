use rusqlite::{types::Value, Connection, OpenFlags};
use std::{path::Path, time::Duration};

fn normalized(sql: &str) -> String {
    sql.to_lowercase()
        .replace("if not exists", "")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ';')
        .collect()
}

fn validate(db: &Connection) -> Result<(), String> {
    let expected = Connection::open_in_memory().map_err(|e| e.to_string())?;
    expected
        .execute_batch(crate::history::SCHEMA)
        .map_err(|e| e.to_string())?;
    expected
        .execute_batch(crate::database::BLOCK_SCHEMA)
        .map_err(|e| e.to_string())?;
    let schema = |connection: &Connection| -> Result<Vec<(String, String, String)>, String> {
        connection
            .prepare("SELECT type,name,coalesce(sql,'') FROM sqlite_master ORDER BY type,name")
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    normalized(&row.get::<_, String>(2)?),
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())
    };
    if schema(db)? != schema(&expected)? {
        return Err("unknown database schema; identity migration refused".into());
    }
    let integrity: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if integrity != "ok" {
        return Err(format!("integrity check failed: {integrity}"));
    }
    if db
        .prepare("PRAGMA foreign_key_check")
        .map_err(|e| e.to_string())?
        .exists([])
        .map_err(|e| e.to_string())?
    {
        return Err("foreign key check failed".into());
    }
    Ok(())
}

fn rows(db: &Connection, table: &str) -> Result<Vec<Vec<Value>>, String> {
    let mut statement = db
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .map_err(|e| e.to_string())?;
    let count = statement.column_count();
    let result = statement
        .query_map([], |row| (0..count).map(|i| row.get(i)).collect())
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string());
    result
}

/// Build a consistent SQLite backup and migrate only the exact Block agent namespace.
/// Callers close all source handles before invoking this function and install the copy separately.
/// Unknown schema, foreign identities and any existing destination namespace are rejected.
pub fn prepare_agent_identity_copy(
    source: &Path,
    destination: &Path,
    old_agent: &str,
    new_agent: &str,
) -> Result<(), String> {
    if old_agent == new_agent || destination.exists() {
        return Err("identity or destination conflict".into());
    }
    let source = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    validate(&source)?;
    let identities: Vec<String> = source
        .prepare("SELECT DISTINCT agent FROM block_definitions")
        .map_err(|e| e.to_string())?
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| e.to_string())?;
    if identities.iter().any(|id| id != old_agent) {
        return Err("unknown source identity or target namespace conflict".into());
    }
    let before_history = rows(&source, "history_records")?;
    let before_fields = rows(&source, "block_fields")?;
    let mut expected_definitions = rows(&source, "block_definitions")?;
    for row in &mut expected_definitions {
        row[1] = Value::Text(new_agent.into());
    }
    let mut copy = Connection::open(destination).map_err(|e| e.to_string())?;
    {
        let backup =
            rusqlite::backup::Backup::new(&source, &mut copy).map_err(|e| e.to_string())?;
        backup
            .run_to_completion(128, Duration::from_millis(5), None)
            .map_err(|e| e.to_string())?;
    }
    copy.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; BEGIN IMMEDIATE;")
        .map_err(|e| e.to_string())?;
    copy.execute(
        "UPDATE block_definitions SET agent=?1 WHERE agent=?2",
        [new_agent, old_agent],
    )
    .map_err(|e| e.to_string())?;
    copy.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    validate(&copy)?;
    if rows(&copy, "history_records")? != before_history
        || rows(&copy, "block_fields")? != before_fields
        || rows(&copy, "block_definitions")? != expected_definitions
    {
        return Err("identity migration changed preserved data".into());
    }
    drop(copy);
    std::fs::File::open(destination)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())
}
