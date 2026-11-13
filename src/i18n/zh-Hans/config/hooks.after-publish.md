# `[[hooks.after-publish]]`

文件发布后运行命令，例如上传站点。

## 配置项

### `enable`

默认启用。设为 `false` 跳过这个 hook。

### `name`

状态和错误消息里显示的名称。同一阶段内不能重名，名称不能为空或包含空白。

### `order`

非负整数，省略时为 `0`。同一站点或模块的 `after-publish` hook 按此数值从小到大执行。同序号的 hook 可以并行。

### `argv`

从站点根目录运行的命令及参数。`argv` 和 `nu` 二选一。通过 `hook.inputDir` 读取已发布的文件。命令出错时，站点已经发布。

### `nu`

要运行的 Nu 脚本，路径相对于站点根目录。`nu` 和 `argv` 二选一。

### `args`

传给 `nu` 脚本的参数，可省略。

### `enabled-by`

例如填 `"images.enable"`，只在 `[options.images]` 的 `enable` 为 `true` 时运行。省略时不检查选项。

### `dev`

省略或设为 `"skip"` 时，`tola dev` 跳过。设为 `"run"` 时，开发构建成功后运行。

### `rerun-on`

开发模式下额外监听的输入。指定后须填 `include`，可省略 `exclude`。例如，用 `{ include = ["tools/upload.toml"] }` 在部署配置变更时触发重建，并让已启用的发布 hook 重新读取它。

## 用法

### 读取发布结果

从 `hook.inputDir` 读取本次发布的只读文件。跨次保留的文件放进 `hook.cacheDir`，只用一次的文件放进 `hook.tempDir`。要给站点添加发布文件，用 `generate-outputs`。

### 开发时什么时候运行

`tola dev` 默认跳过这一步。设 `dev = "run"` 后，页面更新时会在后台运行 hook。如果 hook 还没跑完，期间又完成了多次构建，等它结束后只处理最新的一次。

站点文件、命令和 `rerun-on` 监听的文件都没变时，沿用上次结果，即使上次失败了。退出 `tola dev` 会取消正在运行或还在排队的任务。

示例：上传站点：

确认能运行 `rsync` 后，把下面的脚本保存到站点根目录的 `upload.nu`。把示例目的地换成你有权限写入的目录。源路径末尾的 `/` 告诉 `rsync` 复制目录里的文件。

```nu
use tola get-context

let input_dir = get-context | get hook.inputDir
^rsync -a -- ($input_dir + "/") "user@example.com:/srv/site/"
```

在 `tola.toml` 中加入以下配置。

```toml
[[hooks.after-publish]]
name = "upload"
nu = "upload.nu"
```

从站点根目录运行 `tola build`。Tola 先发布站点，脚本再从 `hook.inputDir` 上传文件。上传失败时，`tola build` 返回错误，本地发布的文件仍在。

示例：检查发布后的首页有没有草稿标记：

在用 `tola init` 创建的站点中，给 `content/index.typ` 写一行 `DRAFT-MARKER`。把下面的脚本存为 `check-home.nu`，与 `tola.toml` 放在一起：

```nu
use tola [get-context diagnostic]

let home = get-context | get hook.inputDir | path join "index.html"

let issues = if (open --raw $home | str contains "DRAFT-MARKER") {
  [
    (
      diagnostic make "Remove DRAFT-MARKER from the home page." --code draft-marker
    )
  ]
} else {
  []
}
$issues | diagnostic submit
```

把 hook 加到 `tola.toml`：

```toml
[[hooks.after-publish]]
name = "check-home"
nu = "check-home.nu"
dev = "run"
```

从站点根目录运行 `tola build`。首页照常发布，Tola 会显示一条关于草稿标记的警告。删掉标记并重新构建，警告就会消失。设了 `dev = "run"`，开发构建发布后也会运行这项检查。
