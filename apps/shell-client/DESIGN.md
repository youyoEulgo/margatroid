# 伪代码格式

```text
板块使用一级标题；方法按签名、行为块和缩进逻辑排列。
method(arguments) -> ReturnType
    行为：
        执行逻辑
-- 仅作简短补充说明
持有关系使用树形图。
```

# 板块定义

```text
# types   src/main.rs   Request与Frame
# handler src/main.rs   Exec与协议输出
# pty     src/pty.rs    Terminal与PTY读写
# output  src/output.rs TerminalOutput
```

# types

```text
Request = Exec(command, max_output_bytes, pty)
        | Terminal(max_output_bytes)
        | Write(text) | Close
Frame = Ready | Result(stdout, stderr, status, truncated, terminal)
      | Output(text, replace) | Exit(eof, exit_code) | Error(message)
```

# handler

```text
request(input: &mut Input) -> Result<Option<Request>>
    行为：
        读取stdin的一行并解析为Request
        EOF返回None

frame(tx: &Frames, value: Value) -> Result<()>
    行为：
        将Frame放入有界队列，由唯一stdout writer顺序输出JSONL
        diagnostic输出到stderr

exec(input: &mut Input, tx: &Frames, script: String, maximum: usize, terminal: bool) -> Result<()>
    行为：
        持有child、stdout、stderr及可选PTY master
        command = /bin/sh -c script
        并发等待child退出并排空stdout、stderr或PTY master
        捕获各流前maximum字节，超出部分继续排空
        正常完成：输出Result
        EOF、close、信号或输出失败：终止进程组，等待child回收
```

# pty

```text
terminal(input: &mut Input, tx: &Frames, maximum: usize) -> Result<()>
    行为：
        持有child、共享PTY master和TerminalOutput
        child = bash --noprofile --norc -i
        并发推进：
            stdin Write：写入master
            master读取：TerminalOutput.process，产生Output
            child.wait：等待直接子进程退出
        child退出：
            取得master当前可读字节数
            将尚未提交的Output送入帧队列
            排出这部分已有尾部字节并输出Output
            输出Exit
        EOF、close、信号或失败：终止进程组，等待child回收
```

# output

```text
TerminalOutput
    parser: VT100 parser
    history: String
    previous: (text, alternate)
    limit: usize

    process(bytes) -> Result<(text, replace)>
        行为：
            逐字节解析，观察滚动并追加history，渲染screen
            alternate或覆写：返回replace
            前缀扩展：返回新增文本
            保留的完整转录超出limit：返回Error
```

# 持有关系

```text
main
├── input、帧sender、唯一stdout writer任务
└── mode
    ├── 直接child
    ├── Terminal读写共享PTY master
    └── Terminal读取独占TerminalOutput

外层调用方
└── 可执行程序生命周期及逃出进程组的后代清理
```
