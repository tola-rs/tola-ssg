#show: config(
  summary: [定义供 Typst 模板、Nu 脚本与 hook 开关共享的自定义站点配置。],
  related: ("@tola/site", "[hooks]", "[modules]"),
)

`[options]` 下的键名完全由站点自行约定。配置值会同步暴露给 Typst（#ref("@tola/site") 的 `site.options`）、Nu 脚本（`use tola get-site; get-site | get options`）以及站点 hook 的 `enabled-by`。

- TOML 日期与时间会自动转为字符串；数字必须为有限值。
- 站点 hook 的 `enabled-by = "banner.enabled"` 会读取 `[options]` 下对应路径的布尔值；模块 hook 则读取各自的 `[modules.<alias>.options]`。
- 通过 #ref("include") 或模块覆盖合并时，嵌套表逐键合并，数组与标量由后加载的值替换。

#example(title: [用同一开关同时控制页面与构建 hook])[
  ```toml
  [options.banner]
  enabled = true
  text = "预览版本"

  [[hooks.generate-outputs]]
  name = "banner"
  nu = "tools/write-banner.nu"
  enabled-by = "banner.enabled"
  outputs = [{ file = "banner.txt" }]
  ```
]
