#show: config(
  summary: [配置站点的公开源地址、部署子路径、标题、作者与语言信息。],
  related: ("@tola/site", "@tola/address"),
)

这些配置会作为 `site` 暴露给 #ref("@tola/site")。填写 `origin` 时，`site.url` 为 `origin` 与 `base-path` 拼接后的完整地址（例如 `"https://example.com/docs/"`）；省略时 `site.url` 为 `none`。

可通过命令行 `--origin` 或 `--base-path` 临时覆盖本次运行的地址与部署前缀。

#example(title: [子路径部署与多语言声明])[
  ```toml
  [site]
  origin = "https://example.com"
  base-path = "/docs/"
  title = "Typst 笔记"
  language = "zh-Hans"
  languages = ["zh-Hans", "en"]
  ```
]

#example(title: [在 Typst 模板中应用站点语言])[
  ```typst
  #import "@tola/site:0.0.0": site
  #set text(
    lang: site.language.lang,
    script: site.language.script,
    region: site.language.region,
  )
  ```
]

#key("origin")[
  站点的协议、主机和可选端口，例如 `"https://example.com"`。部署子路径请写在 `base-path` 中。生成 feed 或 sitemap 时需要填写此项。
]

#key("base-path")[
  站点在域名下的部署前缀，例如 `"/docs/"`。必须以 `/` 开头和结尾。
]

#key("title")[
  站点名称，暴露为 `site.title`，供模板、页面标题与订阅源使用。
]

#key("authors")[
  站点作者列表，每项必填 `name`，可选 `email` 与 `url`，例如 `[{ name = "Alice", email = "alice@example.com" }]`。
]

#key("description")[
  站点简介，暴露为 `site.description`，供页面元数据与订阅源使用。
]

#key("language")[
  站点默认语言。可写 BCP 47 标签（如 `"zh-Hant-TW"`），也可拆写为 `{ lang = "zh", script = "Hant", region = "TW" }`。在 Typst 中暴露为含 `tag`、`lang`、`script`、`region` 的 `site.language`。
]

#key("languages")[
  站点支持的语言列表（如 `["zh-Hans", "en"]`），格式同 `language`。非空时必须包含 `language` 的值；具体多语言路由与切换链接由站点模板自行组织。
]
