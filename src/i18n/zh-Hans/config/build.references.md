# `[build.references]`

对已发布页面中指向空处的链接、资源或 fragment 的检查的诊断级别（检查总是发生）。

## 配置项

### `navigation`

指向站点内其他页面的链接，例如 `<a href="/missing/">`。

### `resources`

页面加载的子资源：图片、样式表、脚本等，例如 `<img src="/assets/missing.png">`。

### `fragments`

链接里的 fragment，例如 `<a href="/page/#missing">`。

## 用法

只有两种地址会被检查：相对地址（例如 `guide/`、`../guide/`，按所在页面的 URL 解析）与站点根地址（例如 `/guide/`，按站点根解析）。写明 scheme 或 host 的地址是外链，构建无法检验——例如 `https://example.com/guide/`。

检查时会读取最终 HTML 里的 URL 属性（`href`、`src` 等），手写 HTML 也算。解析与浏览器一致：相对地址按页面自身的地址解析，页面声明了 `<base href>` 时改按它解析（取第一个）。这个 `<base href>` 指向别的 origin 时，整页相对地址都指向那个站点，Tola 不检查它们，并给出一条警告（`reference.base_href_external_origin`）说明该页未被检查；去掉 `<base href>`，或把它写成相对地址（例如 `/`，站点挂在 `site.base-path` 下时用 `/docs/`）即可。

未解析的引用按类别报出诊断，级别取对应字段：

- `navigation`：链接的目标页面不在 output 里，例如 `<a href="/missing/">`，报 `reference.navigation_missing`
- `resources`：页面加载的资源不在 output 里，例如 `<img src="/assets/missing.png">`，报 `reference.resource_missing`；目标存在但媒体类型不符时，例如 `<script src="/assets/site.css">`，报 `reference.resource_media_mismatch`
- `fragments`：链接点名的 fragment 在目标页面里不存在，例如 `<a href="/guide/#missing">`，报 `reference.fragment_missing`；目标页面本身不存在时，整条引用按 `navigation` 报，fragment 不再单独报告

`#top` 由浏览器内置：URL 里 `#` 后面那段就是 fragment，写 `top` 时浏览器回到页首，页面上不需要 `id="top"`。`#:~:text=words` 这类浏览器文本片段，以及指向非 HTML 文件的 fragment（例如 `/data.json#missing`），都不会报 `reference.fragment_missing`。

这些检查在全部 output 生成完毕之后、发布到 `publish-dir` 之前运行，对照的是本次构建的完整 output：Typst Bundle 编译出的 HTML 页面、`[assets]` 声明的文件、`generate-outputs` hook 写出的文件——链接指向 hook 写出的文件同样有效，不会被报缺失。

`tola check` 与 `tola build`、`tola dev`、`tola preview` 跑同一段检查，只是不替换发布目录。

`@tola/document` 的 `references()` 是独立的原生 Bundle 链接与 ref 查询。它不包含手写 HTML 或其他生产者的 output，`found` 也不保证最终 HTML 定义了该 fragment（`tola help "@tola/document" references`）。
