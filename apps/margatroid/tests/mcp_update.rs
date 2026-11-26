use mlua::{Lua, LuaSerdeExt, Table, Value};
use serde_json::Value as Json;

fn fixture() -> (Lua, Table) {
    let lua = Lua::new();
    lua.globals()
        .set(
            "encode",
            lua.create_function(|lua, value: Value| {
                let value: Json = lua.from_value(value)?;
                Ok(value.to_string())
            })
            .unwrap(),
        )
        .unwrap();
    lua.globals()
        .set(
            "decode",
            lua.create_function(|lua, text: String| {
                let value: Json = serde_json::from_str(&text).map_err(mlua::Error::external)?;
                lua.to_value(&value)
            })
            .unwrap(),
        )
        .unwrap();
    lua.load(r#"
        writes=0; closed={}; fail_save=false
        margatroid={data={null=decode('null')},mcl={},files={},task={}}
        function margatroid.data:encode(format,value) assert(format=='json');return encode(value) end
        function margatroid.data:decode(format,value) assert(format=='json');return decode(value) end
        function margatroid.files:read(path) assert(path=='/resource/input.schema.json');return '{"type":"object"}' end
        config_json='{"initialization":{"custom":[]}}'
        function margatroid.mcl:execute(address,text,args)
            assert(address=='/agent')
            if text=='CONFIG GET' then return {Config=decode(config_json)} end
            assert(text=='CONFIG SET ? ?' and args[1]=='initialization.custom')
            if fail_save then return {Error={message='save failed'}} end
            writes=writes+1; config_json=encode({initialization={custom=args[2]}});return {}
        end
        function margatroid.task:finish(target)
            assert(target.agent=='/agent' and target.task_id=='old')
            closed[#closed+1]=target.task_id
        end
        ctx={agent_info={address='/agent'},alias='mcp_register',task_id='new',resource_dir='/resource'}
        old_args={server={transport='stdio',command='old-command'},alias_prefix='old_'}
        new_args={server={transport='stdio',command='new-command'},alias_prefix='new_'}
    "#).exec().unwrap();
    let source = include_str!("../../../resources/tools/margatroid/mcp-register/dev/main.lua");
    let prefix = source
        .split("local function register_server(")
        .next()
        .unwrap();
    let methods: Table = lua.load(format!("{prefix}\nreturn {{prepare=prepare_registration,persist=persist_registration,replay=make_replay,locate=locate_replay}}" )).eval().unwrap();
    lua.globals().set("methods", methods.clone()).unwrap();
    (lua, methods)
}

#[test]
fn update_replaces_only_exact_original_entry_and_keeps_plain_registration_entry() {
    let (lua, _) = fixture();
    lua.load(r#"
        local old=methods.replay('mcp_register',old_args)
        local other={command='ECHO ?',arguments={'keep'}}
        config_json=encode({initialization={custom={other,old,other}}})
        ctx.binding={kind='mcp_update',agent='/agent',task_id='old',register_alias='mcp_register',replay=old}
        local state={};methods.prepare(new_args,ctx,state)
        assert(#closed==1 and writes==0)
        methods.persist(ctx,state.replay,state.replacing)
        local custom=decode(config_json).initialization.custom
        assert(#custom==3 and custom[1].command=='ECHO ?' and custom[3].command=='ECHO ?')
        local call=custom[2].arguments[1][1]
        assert(call.tool_name=='mcp_register' and call.id=='startup-mcp_register')
        local args=decode(call.arguments)
        assert(args.server.command=='new-command' and args.alias_prefix=='new_')
        assert(not call.arguments:find('task_id',1,true))
    "#).exec().unwrap();
}

#[test]
fn missing_ambiguous_or_wrong_agent_update_never_closes_old_task() {
    for mode in ["missing", "duplicate", "wrong-agent"] {
        let (lua, _) = fixture();
        lua.globals().set("mode", mode).unwrap();
        lua.load(r#"
            local old=methods.replay('mcp_register',old_args)
            ctx.binding={kind='mcp_update',agent='/agent',task_id='old',register_alias='mcp_register',replay=old}
            if mode=='duplicate' then config_json=encode({initialization={custom={old,old}}}) end
            if mode=='wrong-agent' then ctx.binding.agent='/other' end
            local ok=pcall(methods.prepare,new_args,ctx,{})
            assert(not ok and #closed==0 and writes==0)
        "#).exec().unwrap();
    }
}

#[test]
fn save_failure_preserves_old_configuration_and_next_update_can_locate_it() {
    let (lua, _) = fixture();
    lua.load(r#"
        local old=methods.replay('mcp_register',old_args)
        config_json=encode({initialization={custom={old}}});local before=config_json
        ctx.binding={kind='mcp_update',agent='/agent',task_id='old',register_alias='mcp_register',replay=old}
        local state={};methods.prepare(new_args,ctx,state)
        fail_save=true
        assert(not pcall(methods.persist,ctx,state.replay,state.replacing))
        assert(config_json==before and writes==0)
        local binding={kind='mcp_update',replay=state.replay,previous_replay=state.replacing.replay}
        assert(methods.locate(decode(config_json).initialization.custom,binding)==1)
    "#).exec().unwrap();
}

#[test]
fn first_registration_deduplicates_replay_and_only_remote_calls_have_short_deadline() {
    let (lua, _) = fixture();
    lua.load(
        r#"
        assert(configure({},ctx).timeout_ms==0)
        local replay=methods.replay('mcp_register',old_args)
        methods.persist(ctx,replay,nil);assert(writes==1)
        methods.persist(ctx,replay,nil);assert(writes==1)
        ctx.binding={kind='mcp_tool'};assert(configure({},ctx).timeout_ms==30000)
        ctx.binding={kind='mcp_update'};assert(configure({},ctx).timeout_ms==0)
    "#,
    )
    .exec()
    .unwrap();
}
