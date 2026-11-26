#show: config(
  summary: [配置 `tola dev` 的文件监听与自动刷新行为。],
  related: ("[server]", "[hooks]"),
)

`tola dev` 会按各 hook 的 `dev` 配置执行；若要本地预览生产构建，请使用 `tola preview`。单次关闭监听可传 `tola dev --watch=false`。

调试 hook 时，Tola 自动监听 `nu` 入口脚本；若脚本读取了额外数据文件，将其加入 `rerun-on` 即可在变更时触发重建。传入 `tola dev --no-reuse` 可强制重跑配置了 `dev = "reuse"` 的 hook。

#example(title: [关闭文件变更自动重建])[
  ```toml
  [dev]
  watch = false
  ```
]

#key("watch")[
  是否在监听文件变更时自动重建站点并刷新浏览器。
]
