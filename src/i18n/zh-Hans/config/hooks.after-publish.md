# `[[hooks.after-publish]]`

通过只读视图消费一个已提交的 revision，不再声明站点输出。

## 配置项

### `enable`

是否运行该 hook。

### `name`

该 hook 的名字，出现在状态行与诊断里。必须是一个不含空白的词，且在本阶段内唯一：为空、含空白、或在本阶段内重名都会报错。

### `argv`

运行的命令，从站点根执行，不经过隐式 shell。`TOLA_HOOK_INPUT_DIR` 是已发布站点的一份只读视图，因此这里失败不会撤销发布。

### `nu`

嵌入式 Nu 脚本，相对于站点根。

### `args`

传给 Nu 脚本的字面参数。

### `enabled-by`

启用该 hook 的站点顶层布尔选项。

### `dev`

`tola dev` 是否运行该 hook：`"skip"`（默认）或 `"run"`。

### `rerun-on`

额外观察的站点相对模式，使用必填的 `include` 与可选的 `exclude`。所选输入变化触发一次完整的 `tola dev` 构建。

## 用法

这个阶段消费已成功提交的站点，例如把它同步到主机：

`justfile`：

```just
rsync:
    rsync -a --delete "$TOLA_HOOK_INPUT_DIR/" user@example.com:/srv/site/
```

`tola.toml`：

```toml
[[hooks.after-publish]]
name = "rsync"
argv = ["just", "rsync"]
```

上面的 recipe 把刚发布的站点同步到主机。after-publish 在发布完成之后运行：`TOLA_HOOK_INPUT_DIR` 是一份只读快照，就是刚发布的那个站点，命令读取期间不会变。该阶段的 hook 执行时，视作先前构建已经成功：命令失败不会撤销发布（站点的构建结果已经写入 `build.publish-dir`）。这个阶段是发布之后的收尾：hook 可以用这份站点做事（上传、通知等）；它在这里写出的文件不属于这次发布，也不会被检查。要让站点多出文件，请在 `generate-outputs` 里生成——那里的输出会在发布前被检查，并随站点发布。

`tola build` 等这些命令完成才退出；即使 output 已写出，消费者失败仍使命令结果失败。`check`、`preview`、`inspect` 与 `vendor` 从不运行它们。

与所有 hook 阶段一样，这里的命令也只跑单次：开发监听由 Tola 负责，命令自己的增量与缓存由生产者自己编写（可用 `TOLA_HOOK_CACHE_DIR`）。

开发模式默认跳过此阶段。设 `dev = "run"` 后，消费者串行运行，不阻塞浏览器更新。一个 revision 运行期间，只保留最新的等待 revision，因此中间 revision 可能没有消费者调用。重建如果没有带来任何变化（站点、命令与 `rerun-on` 内容都与上一次运行相同），就沿用上一次运行的结果（包括失败），不再重新运行命令。停止 `tola dev` 会取消正在运行的命令，并丢弃等待任务。这个模式适合处理最新站点的消费者，不适合审计每一个 revision。

本阶段的命令环境：
- `TOLA_HOOK_INPUT_DIR`：命令的输入——刚提交的那个站点的只读快照，布局与发布后的目录一致，整棵就是要上传的站点。
- `TOLA_HOOK_STAGE`、`TOLA_BUILD_MODE`、`TOLA_HOOK_CACHE_DIR`、`TOLA_HOOK_TEMP_DIR`：与其它阶段相同——阶段名、模式（`prod` 或 `dev`）、跨次缓存目录、本次临时目录。

没有 `TOLA_HOOK_OUTPUT_DIR`：本阶段不添加 output。
