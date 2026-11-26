#show: config(
  summary: [配置站点的 Typst 入口文件、内容目录、发布目录及 hook 并发策略。],
  related: ("[assets]", "[hooks]", "@tola/source"),
)

入口文件编译为一个 Typst Bundle，负责挑选内容源、渲染页面，并通过 `#document(...)` 指定每个输出在 `publish-dir` 下的相对路径。

内容发现会自动跳过入口文件；共用模板和辅助脚本请放在 `content-dir` 之外。所有输出（文档、#ref("[assets]") 与 #ref("[[hooks.generate-outputs]]")）在替换 `publish-dir` 前都会统一校验；若编译、校验或发布前 hook 失败或被取消，上次发布的文件保持不变。

#example(title: [默认构建目录配置])[
  ```toml
  [build]
  entry = "site.typ"
  content-dir = "content"
  publish-dir = "public"
  ```
]

#example(title: [发布全部发现的内容文件])[
  ```typst
  #import "@tola/address:0.0.0": join-route, output-of
  #import "@tola/source:0.0.0": all-sources

  #for source in all-sources() {
    document(output-of(join-route(source.route-segments)), format: "html")[
      #include source.file
    ]
  }
  ```
]

#key("entry")[
  站点根 Typst 入口文件路径（相对于站点根目录），或使用 `{ module = "alias" }` 采用指定模块提供的入口。
]

#key("content-dir")[
  `all-sources()` 扫描的内容目录（相对于站点根目录），必须位于 `.tola` 之外。
]

#key("publish-dir")[
  `tola build` 的发布目录（相对于站点根目录）。请选择空目录或本站上次发布的目录——构建成功后会替换该目录的全部内容。
]

#key("par-hooks")[
  允许同一阶段、同一站点或模块实例且 `order` 相同的 hook 并行执行。设为 `false` 时逐个顺序执行，批次顺序不变。
]

#key("hook-jobs")[
  单次会话中跨所有模块与阶段同时运行的 hook 上限。可填 `"auto"`（启动时按本机可用并行度决定）或正整数；不限制脚本自身创建的线程数。
]
