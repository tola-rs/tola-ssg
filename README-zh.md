# tola-ssg

为 Typst 网站而生的静态网站生成器。

> 注意（v0.7.x）：已发布。可能仍存在一些与缓存相关的 bug。你可以使用 `tola s -c`（`serve --clean`）作为临时解决方案，但请先尝试普通的 `serve` 以便我收集反馈并在后续更新中修复这些问题。感谢你的支持！


## 目录

- [展示](#展示)
- [特性](#特性)
- [设计理念](#设计理念)
- [使用说明](#使用说明)
- [安装](#安装)
- [社区](#社区)
- [注意事项](#注意事项)
- [致谢](#致谢)

## 展示

> 是的，我的博客也是用 `tola` 搭建的。

| 网站 | 描述 |
|------|-------------|
| [kawayww.com](https://kawayww.com) | 作者的个人博客 |
| [example-sites](https://tola-rs.github.io/example-sites/) | 官方示例集合 |

**我的网站（[kawayww.com](https://kawayww.com)）**

| | |
|:---:|:---:|
| <img src="screenshots/home-0.webp" width="100%"> | <img src="screenshots/home-1.webp" width="100%"> |
| <img src="screenshots/home-2.webp" width="100%"> | <img src="screenshots/home-3.webp" width="100%"> |

**初始模板**（[example-sites/starter](https://tola-rs.github.io/example-sites/starter)）

| | |
|:---:|:---:|
| <img src="screenshots/starter-0.webp" width="100%"> | <img src="screenshots/starter-1.webp" width="100%"> |

<details>
<summary>如何使用 Tola 的虚拟包系统实现"最近5篇文章"</summary>

得益于 `typst` 和 `tailwindcss`，`tola` 提供了灵活的写作方式。
使用 `@tola/pages` 虚拟包可以轻松实现「最近文章」功能。
此代码片段对应的初始模板虚拟包文章源码位于：
`https://github.com/tola-rs/example-sites/blob/main/starter/content/posts/virtual-packages.typ`。

```typst
#import "@tola/pages:0.0.0": pages
#import "/components/ui.typ" as ui

#let posts = (pages()
  .filter(p => "/posts/" in p.permalink)
  .filter(p => p.at("date", default: none) != none)
  .sorted(key: p => p.date)
  .rev())

#html.div(class: "space-y-6")[
  #for post in posts.slice(0, calc.min(posts.len(), 5)) {
    ui.post-card(post)
  }
]
```

`@tola/pages` 包可在编译时访问所有页面元数据（标题、日期、永久链接、标签等）。

</details>

## 特性

### 性能

- **并行编译** — 并发处理页面
- **字体预加载** — 字体在启动时加载一次，所有编译过程共享
- **快照共享** — Typst 编译器快照在批量编译中复用，避免重复初始化

### 开发体验

- **零配置启动** — `tola init <SITE-NAME>` 几秒内即可运行
- **本地服务器** — 内置 HTTP 服务器，按需编译
- **热重载** — 文件变更通过 WebSocket 实时 diff/patch 到浏览器
- **优先级队列调度器** — 优先处理当前查看的页面，反馈更快
- **增量重建** — 双向依赖图 + VDOM 缓存实现最小化重建；仅重新编译受影响的页面
- **优雅的错误处理** — 来自 Typst 的可读性诊断消息
- **逃生舱** — 当你需要时可以完全访问 HTML/CSS/JS

### 构建与集成

- **构建钩子** — 前后构建钩子用于自定义脚本（如 esbuild、imagemin）
- **Tailwind CSS** — 内置 CSS 处理器集成
- **HTML/XML 压缩** — 可选的生产构建压缩
- **SPA 导航** — 可选的客户端导航，带 DOM 变形和 View Transitions API（限制：内联脚本应该是幂等的；导航可能会执行多次）

### 路由与 SEO

- **简洁友好的 URL** — `content/posts/hello.typ` → `/posts/hello/`
- **自定义永久链接** — 通过页面元数据覆盖 URL
- **别名** — 将旧 URL 重定向到新位置
- **URL 短链接化** — 可配置的短链接模式（full、safe、ascii），支持大小写选项
- **URL 冲突检测** — 多个页面解析到同一 URL 时报错
- **RSS/Atom 支持** — 根据页面元数据自动生成 `feed.xml`
- **站点地图** — 为搜索引擎自动生成 `sitemap.xml`
- **Open Graph 与 Twitter Cards** — 根据站点配置自动注入默认 OG 标签，或通过 Typst 中的 `og-tags()` 为每个页面自定义
- **404 typst/html 页面** — 可配置的未找到页面（.typ 或 .md）

### 虚拟包

Tola 在编译时注入虚拟包，无需外部构建步骤即可实现跨页面数据访问：

- `@tola/site:0.0.0` — 站点元数据和根路径
- `@tola/pages:0.0.0` — 所有页面元数据（标题、日期、永久链接、标签、草稿状态等）
- `@tola/current:0.0.0` — 当前页面上下文（`current-permalink`、`path`、`headings`、导航辅助函数……）

```typst
#import "@tola/pages:0.0.0": pages
#import "@tola/site:0.0.0": info, root

// 列出所有文章
#for post in pages().filter(p => "/posts/" in p.permalink) {
  [#post.title (#post.date)]
}

// 访问站点标题
#info.title
```

规范示例维护在初始模板文章中：
`https://github.com/tola-rs/example-sites/blob/main/starter/content/posts/virtual-packages.typ`

更多详情请参阅[使用说明中的虚拟包](#虚拟包)。

## 设计理念

> **保持专注于内容本身。**

### Typst 优先

如果 Typst 能轻松完成，那就用 Typst。不必在这里赘述 Typst 的优势——即使 HTML 导出会丢失许多布局特性，它仍然非常强大。

`tola` 利用 Typst 的标记和脚本能力，而不是重新发明轮子。

### Tola 其次

有些事情超出了独立的 `typst` CLI 能做的范围——尤其是批量处理和站点级协调：

- 根据文件结构自动路由
- 带 VDOM diff/patch 的无缝热重载
- 开箱即用的 SVG 暗色模式适配
- 通过 `sys.inputs` 和虚拟包注入实现跨页面状态
- ……还有更多！

这就是 `tola` 的用武之地——优化开发者体验并无缝集成这些功能并非易事。


## 使用说明

- [示例站点结构](#示例站点结构)
- [共享依赖](#共享依赖)
- [配置](#配置)
- [虚拟包](#虚拟包)
- [Open Graph 与 Twitter Cards](#open-graph--twitter-cards)
- [快速开始](#快速开始)

运行 `tola --help` 或 `tola <command> --help` 查看详细的 CLI 用法。

你可以从任何子目录运行 `tola`——它会自动向上搜索 `tola.toml`。

### 示例站点结构

```text
.
├── tola.toml                 # 站点配置
├── content/                  # 页面源文件（路由）
│   ├── index.typ             #   -> /
│   ├── about.typ             #   -> /about/
│   ├── posts/
│   │   └── hello.typ         #   -> /posts/hello/
│   └── error.typ             # 自定义 404 页面
├── templates/                # 共享布局（默认在 `build.deps` 中）
│   ├── tola.typ              #   来自 `tola init` 的默认模板（完全可自定义）
│   ├── post.typ              #   文章布局（可以继承 tola.typ）
│   └── normal.typ            #   常规页面布局
├── utils/                    # 辅助函数（默认在 `build.deps` 中）
│   └── tola.typ              #   来自 `tola init` 的工具函数（CSS 类、OG 标签等）
├── components/               # 自定义组件（需要手动添加到 `build.deps`）
│   ├── layout.typ            #   可复用的布局组件
│   └── ui.typ                #   UI 组件（post-card、tag-list 等）
└── assets/
    ├── images/
    ├── fonts/
    │   └── Luciole-math.otf  # 内嵌数学字体（tola 自动加载）
    ├── styles/
    │   └── tailwind.css      # Tailwind 输入（如果使用 `build.hooks.css`）
    └── scripts/
```

### 共享依赖

`content/` 下的路由可能很直观——文件映射到 URL。但你可能想知道 `tola.toml` 中的 `build.deps`。实际上你可以不用想太多就用它，但快速解释一下可能有帮助：

`content/` 中的 Typst 文件会变成页面。但它们经常从 `templates/`、`utils/` 或你喜欢的其他位置 `#import` 共享代码——这些只是 tola 默认提供的常规名称，可以随意重命名。Tola 内部跟踪这些依赖。当你在 `build.deps` 中声明目录时，tola 知道："如果这里有任何变化，重新编译所有从它导入的页面。"这实现了整个站点的即时热重载。

`templates/` 和 `utils/` 只是默认名称——你可以通过 `build.deps` 重命名它们或添加更多。例如：你有一个 `templates/base.typ` 用 Tailwind 类设置数学公式样式。当你在该文件中把 `text-base` 改成 `text-2xl` 时，任何导入它的页面（如 `content/example.typ` -> `/example/`）都会立即反映更大的公式——无需手动刷新。

### 配置

常见的 `tola.toml` 设置（运行 `tola init --dry` 查看完整默认值）：

```toml
# 在 Typst 中访问：#import "@tola/site:0.0.0": info
# 然后使用：info.title, info.author, info.extra.custom
[site.info]
title = "My Blog"
author = "Your Name"
email = "you@example.com"
description = "A blog built with Typst and Tola"
language = "en"
url = "https://example.com"

[site.info.extra]
custom = "This is my custom data"

[site.header]
icon = "assets/images/favicon.ico"
styles = ["assets/styles/custom.css"]
scripts = [
  "assets/scripts/custom.js" # 简单：无 defer 和 async
  { path = "assets/scripts/app.js", defer = true }
  { path = "assets/scripts/app.js", async = true }
]
elements = ['<meta name="darkreader-lock">'] # 额外的特殊 html 元素

[site.seo]
auto_og = true   # 自动注入默认 OG 标签（site_name、locale、description、type、twitter:card）

[site.seo.feed]
enable = true
format = "rss"   # "rss" | "atom"

[site.seo.sitemap]
enable = true

[build]
content = "content"
output = "public"
minify = true
deps = ["templates", "utils"]  # 共享依赖 — 变更触发范围重建

[build.assets]
nested = ["assets/images", "assets/styles", "assets/fonts"]

[build.hooks.css]
enable = true
path = "assets/styles/tailwind.css"
command = ["tailwindcss"]
```

### 虚拟包

Tola 提供可以在 Typst 文件中直接导入的虚拟包。

重要提示：请以初始模板文章作为 API 名称和示例的权威来源。不要在多个地方维护独立的手写变体。

- 展示（渲染输出）：
  [`tola-rs.github.io/example-sites/starter/posts/virtual-packages/`](https://tola-rs.github.io/example-sites/starter/posts/virtual-packages/)
- 源文件：
  [`tola-rs/example-sites/starter/content/posts/virtual-packages.typ`](https://github.com/tola-rs/example-sites/blob/main/starter/content/posts/virtual-packages.typ)
- 初始模板仓库：
  [`github.com/tola-rs/example-sites/tree/main/starter`](https://github.com/tola-rs/example-sites/tree/main/starter)

| 包 | 导出 |
|---------|---------|
| `@tola/site:0.0.0` | `info` — 站点元数据（标题、作者、邮箱、描述、url、语言、版权、extra）；`root` — 站点根路径 |
| `@tola/pages:0.0.0` | `pages()`、`by-tag(tag)`、`by-tags(..tags)`、`all-tags()` |
| `@tola/current:0.0.0` | `current-permalink`、`parent-permalink`、`path`、`filename`、`links-to`、`linked-by`、`headings`、`siblings(pages)`、`children(pages)`、`breadcrumbs(pages, include-root: false)`、`at-offset(sorted-pages, offset)`、`prev(sorted-pages, n: 1)`、`next(sorted-pages, n: 1)`、`take-prev(sorted-pages, n: 1)`、`take-next(sorted-pages, n: 1)` |

```typst
// content/index.typ — 列出最近文章
#import "@tola/pages:0.0.0": pages

#let posts = (pages()
  .filter(p => "/posts/" in p.permalink)
  .filter(p => p.at("date", default: none) != none)
  .sorted(key: p => p.date)
  .rev())

#let recent = posts.slice(0, calc.min(posts.len(), 5))

#for post in recent {
  [- #link(post.permalink)[#post.title]]
}
```

<details>
<summary>示例：最近文章</summary>

```typst
#import "@tola/pages:0.0.0": pages

#let posts = (pages()
  .filter(p => "/posts/" in p.permalink)
  .filter(p => p.at("date", default: none) != none)
  .sorted(key: p => p.date)
  .rev())

#let recent = posts.slice(0, calc.min(5, posts.len()))

#for post in recent {
  [- #link(post.permalink)[#post.title]]
}
```

</details>

<details>
<summary>示例：从文件名派生元数据</summary>

使用 `@tola/current` 中的 `path` 和 `filename` 解析类似 `2025_02_27_hello.typ` 的文件名中的日期：

```typst
#import "@tola/current:0.0.0": path, filename

#let file = filename.replace(".typ", "").replace(".md", "")
#let parts = file.split("_")
#let auto-date = if parts.len() >= 4 {
  parts.slice(0, 3).join("-")
} else {
  none
}
```

</details>

<details>
<summary>示例：层级结构 + 导航辅助函数</summary>

```typst
#import "@tola/pages:0.0.0": pages
#import "@tola/current:0.0.0": prev, next, breadcrumbs, children, siblings

#let all = pages()
#let sorted-posts = (all
  .filter(p => "/posts/" in p.permalink and p.date != none)
  .sorted(key: p => p.date))

#let prev-post = prev(sorted-posts)
#let next-post = next(sorted-posts)
#let crumbs = breadcrumbs(all, include-root: true)
#let direct-children = children(all)
#let same-level = siblings(all)
```

</details>

<details>
<summary>示例：偏移量导航窗口</summary>

```typst
#import "@tola/pages:0.0.0": pages
#import "@tola/current:0.0.0": at-offset, take-prev, take-next

#let dated = (pages()
  .filter(p => "/posts/" in p.permalink and p.date != none)
  .sorted(key: p => p.date))

#let two-back = at-offset(dated, -2)
#let two-forward = at-offset(dated, 2)
#let previous = take-prev(dated, n: 2)
#let next = take-next(dated, n: 2)
```

</details>

### Open Graph 与 Twitter Cards

当 `site.seo.auto_og = true` 时，Tola 会自动从 `[site.info]` 注入默认 OG 标签。要为特定页面自定义，请在模板的 `head` 参数中使用 `og-tags()` 函数：

```typst
#import "/templates/tola.typ": tola-page
#import "/utils/tola.typ": og-tags, parse-date

#let head = og-tags(
  title: "My Post",
  description: "A great article about...",
  url: "https://example.com/posts/my-post/",
  image: "https://example.com/og-image.png",
  type: "article",                      // "website" | "article" | "book" | "profile"
  published: parse-date("2024-01-15"),  // article:published_time
  tags: ("rust", "typst"),              // article:tag
)

// 在你的模板中
tola-page(
  title: "My Post",
  head: head,
)[...]
```

当你使用 `og-tags()` 时，Tola 会跳过自动注入，改用你的自定义标签。

### 快速开始

```sh
# 创建新站点
tola init my-blog
cd my-blog

# 编辑 `content/index.typ`

# 为生产环境构建
tola build

# 启动开发服务器
tola serve
```

## 安装

### Cargo

```sh
cargo install --locked tola
```

### 二进制发布版

从 [发布页面](https://github.com/tola-rs/tola-ssg/releases) 下载。

### Nix Flake

仓库中提供了 `flake.nix`。预构建的二进制文件可在 [tola.cachix.org](https://tola.cachix.org) 获取。

**第一步**：在你的 `flake.nix` 中将 tola 添加为输入：

```nix
{
  inputs.tola = {
    url = "github:tola-ssg/tola-ssg/v0.7.1";
    inputs.nixpkgs.follows = "<your nixpkgs input here>";
    inputs.rust-overlay.follows = "<your rust-overlay input here, if you have one>";
    # ...
  };
}
```

**第二步**：在 `configuration.nix` 中配置 cachix：

```nix
{ config, pkgs, inputs, ... }:

{
  nix.settings = {
    substituters = [ "https://tola.cachix.org" ];
    trusted-public-keys = [ "tola.cachix.org-1:5hMwVpNfWcOlq0MyYuU9QOoNr6bRcRzXBMt/Ua2NbgA=" ];
  };

  environment.systemPackages = [
    # 1. 本地构建（如果你想从源码构建，推荐）
    # inputs.tola.packages.${pkgs.system}.default

    # 2. 预构建二进制文件（推荐用于快速 CI/CD）
    # 选择与你系统匹配的版本：
    inputs.tola.packages.${pkgs.system}.aarch64-darwin        # macOS (Apple Silicon)
    # inputs.tola.packages.${pkgs.system}.x86_64-linux        # Linux (x86_64)
    # inputs.tola.packages.${pkgs.system}.aarch64-linux       # Linux (ARM64)
    # inputs.tola.packages.${pkgs.system}.x86_64-windows      # Windows (x86_64)

    # 3. 静态二进制文件（仅限 Linux）
    # inputs.tola.packages.${pkgs.system}.x86_64-linux-static
    # inputs.tola.packages.${pkgs.system}.aarch64-linux-static
  ];
}
```

如果你在 nix 沙箱中需要额外的 typst 包（网络不可用）：

```nix
inputs.tola.packages.${pkgs.system}.default.withPackages (ps: [ ps.metalogo ])
```

它为 `tola` 设置 `TYPST_PACKAGE_CACHE_PATH`，因此用户可以通过 `@preview/...` 使用包。
（`tola` 本身完全不依赖 typst CLI）

## 社区

- Matrix: [`#tola:matrix.org`](https://matrix.to/#/#tola:matrix.org)
- QQ: `1065579014`

## 注意事项

> **早期开发阶段 & 实验性 HTML 导出**

`tola` 可用但在不断演进——预期会有破坏性变更和不足之处。欢迎反馈和贡献！

Typst 的 HTML 输出尚未如 PDF 输出那样成熟。部分功能需要变通方案：

- **数学渲染** — 公式导出为内联 SVG，可能需要 CSS 调整以获得正确的大小和对齐（[issue #24](https://github.com/tola-rs/tola-ssg/issues/24)）
- **空白处理** — Typst 在内联元素之间插入 `<span style="white-space: pre-wrap">` 以保留间距（[PR #6750](https://github.com/typst/typst/pull/6750)）
- **布局** — 某些 Typst 布局原语无法完美转换为 HTML 语义

这些是 Typst 本身的上游限制，而非 `tola`。随着 Typst 的 HTML 后端成熟，这些不足之处将逐渐消除。

## 文档

- 运行 `tola --help` 和 `tola <command> --help` 查看 CLI 用法
- 参考 [tola-rs/example-sites](https://github.com/tola-rs/example-sites) 中的示例和源码
- 如有任何问题，请提交 issue

更多正式文档将陆续推出。

# 致谢

- [typsite](https://github.com/Glomzzz/typsite): 为 typst 打造的静态网站生成器（SSG）
- [kodama](https://github.com/kokic/kodama): 面向 Typst 的静态 Zettelkästen 站点生成器。

## 许可证

MIT
