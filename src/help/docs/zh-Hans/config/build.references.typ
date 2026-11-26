#show: config(
  summary: [配置站内无效链接、缺失资源或未知锚点的诊断级别（报错或警告）。],
)

Tola 在发布前校验所有 HTML 输出中的站内引用。绝对 URL（包括与 `site.origin` 相同的完整地址）按外部链接处理，不参与站内校验。

#example(title: [允许缺失页面链接降级为警告])[
  ```toml
  [build.references]
  navigation = "warn"
  ```
]

#key("navigation")[
  站内链接指向不存在的路由时的诊断级别。
]

#key("resources")[
  引用的图片、样式表、脚本等资源不存在或媒体类型不匹配时的诊断级别。
]

#key("fragments")[
  目标页面存在，但 `#fragment` 锚点未出现在目标 HTML 中时的诊断级别。
]
