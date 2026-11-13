# `[vendor]`

把依赖复制到站点里，供离线或 `--pure` 构建使用。支持远程模块、Typst 包和远程图标集。

## 配置项

### `path`

`tola vendor` 将依赖复制到这里。运行命令前，在 `tola.toml` 里设置一个相对于站点根目录的路径。

## 用法

例如，把依赖放进 `vendor/`：

```toml
[vendor]
path = "vendor"
```

运行 `tola vendor`，再用 `tola build --pure` 确认能用这些副本构建。把 `vendor/` 和 `tola.lock` 随站点提交。Tola 会先校验准备好的副本，再替换这三个子目录：

- `modules/` 按别名保存锁定的远程模块。
- `typst-packages/` 按命名空间、包名和版本保存 Typst 包。
- `icons/` 为每个远程图标集保存一个 JSON 文件。

编译字体用 `[typst.fonts] paths` 指定目录，`tola vendor` 不会复制它们。

`path` 必须位于站点内、`.tola` 之外。准备失败时保留上次副本。替换中断后，先重新运行 `tola vendor` 再构建。

用 `tola vendor --dry-run` 只准备和校验副本。要从配置的来源重新复制，用 `tola vendor --refresh`。

如果站点需要 `before-build` 生成的文件，先生成文件，再运行 `tola vendor`。用 `tola check` 检查站点链接，包括指向 hook 输出的链接。

`--offline` 禁止 Tola 联网。`--pure` 还会拒绝读取全局包、缓存、系统字体和站点外的路径，符号链接指到站点外也会拒绝。仍可使用 Tola 内置的包和字体。hook 自己运行的工具仍使用原有权限。
