# `[build.references]`

站内链接、资源或锚点有问题时，选择报错或发警告。

## 配置项

### `navigation`

目标页面找不到时，默认报错。设为 `"warn"` 只发警告。

### `resources`

资源找不到或媒体类型不对时，默认报错。设为 `"warn"` 只发警告。

### `fragments`

页面存在，但链接里 `#` 指定的锚点找不到时，默认报错。设为 `"warn"` 只发警告。

## 用法

例如，允许站点发布时仍有链接指向缺失的页面：

```toml
[build.references]
navigation = "warn"
```

在用 `tola init` 创建的站点中，把这行写入 `content/index.typ`。无需创建 `/missing/` 页面。

```typst
#link("/missing/")[Read the guide]
```

从站点根目录运行 `tola check`。终端会为 `/missing/` 显示一条 `reference.navigation_missing` 警告。

Tola 从最终的 HTML 中读取 `href`、`src` 等属性，手写的 HTML 也会检查。它检查 `../guide/` 和 `/guide/` 这样的站内地址，跳过带协议或主机名的地址，例如 `https://example.com/guide/`。

相对地址以当前页面的 URL 或该页第一个 `<base href>` 为准。如果 `<base href>` 指向站外，Tola 会跳过这一页的相对地址并发出 `reference.base_href_external_origin` 警告。想检查这些链接，可以移除 `<base href>`，或改成站内地址，例如 `/` 或 `/docs/`。

| 配置项 | 问题 | 诊断码 |
| --- | --- | --- |
| `navigation` | 目标页面不存在 | `reference.navigation_missing` |
| `resources` | 资源文件不存在 | `reference.resource_missing` |
| `resources` | 媒体类型不符，例如 `<script src="/assets/site.css">` | `reference.resource_media_mismatch` |
| `fragments` | 目标页面存在，但缺少指定的锚点 | `reference.fragment_missing` |

找不到目标页面时，只报告 `navigation`。浏览器会处理 `#top` 和 `#:~:text=words`，Tola 会跳过这两种地址，也会跳过非 HTML 文件地址中的锚点，例如 `/data.json#missing`。

`tola check`、`tola build`、`tola dev` 和 `tola preview` 都检查这些引用。检查时会把 Typst 页面、`[assets]` 文件和 `generate-outputs` 写出的文件一起列入可用目标。只想检查，运行 `tola check`。

在模板里查询 Typst 的 `link` 和 `ref`，用 `@tola/document` 的 `references()`。检查最终 HTML，用 `tola check`。
