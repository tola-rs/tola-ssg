# `[vendor]`

把 packages、字体与图标随站点一起保留，以便在其他机器上离线构建。

## 配置项

### `path`

`tola vendor` 把这次构建选中的输入冻结到哪个目录，相对站点根。

## 用法

`tola vendor` 把生产构建选中的外部输入冻结到这个目录。使用命令之前声明 `path`：

```toml
[vendor]
path = "vendor"
```

`path` 必须是站点根下、`.tola` 之外的真实目录。

Tola 准备依赖，检查准备好的副本能否在 `--pure` 下构建，然后才替换已安装副本。准备失败会保留上次输入；替换中途中断时，下一次 `tola vendor` 会完成替换，在那之前构建会拒绝运行，`--dry-run` 也会拒绝运行。Tola 的工作文件放在 `path` 旁的兄弟目录里，其名字以 `.` 开头、以 `-swap` 结尾：上面的 `vendor` 对应 `.vendor-swap`。

`tola vendor` 不运行任何 hook 阶段——`before-build`、`generate-outputs`、`after-publish` 都不跑——也不发布站点 output。`before-build` hook 生成的文件必须已经存在：先用一次正常构建把它们生成出来。`generate-outputs` hook 的文件也不在冻结范围内：每次构建都会重新生成，本机如此，用 vendored 输入构建的机器也如此。

Tola 整体替换这两个子目录，`path` 下的其他文件不动：

- `typst-packages/` —— 按 `{namespace}/{name}/{version}/` 存放解析到的 package，包括从宿主根目录或 `--package-path` 选中的 package
- `icons/` —— 每个配置的 `remote-json` collection 对应一个 `<namespace>.json`

站点自己的源文件要放在这两个目录之外。所有 namespace 的 package，包括 `@preview`，都先从 vendor 解析，再查宿主根目录；远程图标 collection 优先读取 vendored 副本——这也是 `--pure` 构建读取它的唯一来源。把这些输入随站点提交：

```sh
tola vendor --dry-run   # prepare and verify without replacing vendor
tola vendor             # install the verified dependencies
tola build --pure       # build from site-owned inputs
```

`--refresh` 不读取已安装的 vendor 副本：每个输入都从其余可用来源（宿主 package 根目录、缓存、网络等）重新解析一遍。用它替换过时的副本，或解析 `tola doctor` 报告的符号链接。重新解析不到某个输入时，保留上次副本。`--dry-run` 保持该副本不变，但可能获取依赖、更新缓存，或等待另一个站点命令完成。

`--offline` 拒绝 Tola 的网络请求，但允许宿主 package 根目录与缓存，以及已开启的系统字体。`--pure` 还排除这些宿主输入与物理上位于站外的源文件，包括指向站外的符号链接。嵌入 package 与字体仍然可用；图像的派生缓存也可使用，因为其源字节仍能复现结果。

这些标志限制 Tola 读取输入，不限制 hook 命令及其工具链：整个构建要在其他机器上复现，需要自己固定每个 hook 的命令及其工具链读取的文件（清单与锁文件），或者提交它生成的文件。
