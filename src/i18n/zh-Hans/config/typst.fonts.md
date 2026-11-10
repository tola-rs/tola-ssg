# `[typst.fonts]`

Typst 编译器可以使用的字体。编译器字体不会被发布：网页字体请在 `[assets]` 下声明。

## 配置项

### `paths`

要搜索的字体目录，相对站点根，递归查找。

### `system`

是否把宿主机自己的字体也算上。开启后，换一台机器构建可能得到不同的结果。

## 用法

编译器字体用于 Typst 内容的排版与导出，字体文件本身不会被发布。`paths` 列出相对站点根的目录，递归查找字体。默认值是 `static/typst-fonts`：

```toml
[typst.fonts]
paths = ["static/typst-fonts"]
system = false
```

vendored 字体与这些目录一起加载，均先于系统字体发现。Tola 也自带嵌入字体，`--pure` 下仍然可用。调用允许时，`system = true` 加上宿主字体；`--pure` 即使在这个设置开启时也排除它们。需要另一台机器使用相同字体输入构建时，保持关闭。

命令行的 `--font-path DIR` 为这一次调用额外加入一个宿主机目录，排在 `paths` 之前；`--ignore-system-fonts` 同样只收窄这一次调用。

浏览器字体是另一回事：通过 `[assets]` 发布，再用 CSS `@font-face` 加载。Typst 字体设置不会为浏览器安装字体：

```toml
[assets]
trees = [{ source = "static/web-fonts", url-prefix = "/fonts" }]
```
