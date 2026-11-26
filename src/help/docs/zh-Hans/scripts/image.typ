#show: script(
  summary: [查看图片元数据，或将本地图片缩放、裁剪并转换格式。],
)

#command(
  "main",
  summary: [列出子命令（`metadata` 与 `resize`）。],
)

#command(
  "metadata",
  summary: [以 JSON 输出图片的像素宽高（`width`、`height`）、格式（`format`）及透明通道信息（`has-alpha`）。],
  params: (
    source: [图片文件路径，相对于当前工作目录解析。],
  ),
)[
  #example(title: [查看本地 PNG 图片信息])[
    ```sh
    tola nu @tola-scripts/image metadata photo.png
    ```
  ]
]

#command(
  "resize",
  summary: [缩放、裁剪或转换本地图片（支持 SVG），输出为新文件或原始字节。],
  params: (
    source: [源图片路径，支持常见位图及 SVG。],
    "--output": [在已有目录下创建新文件保存结果，并以 JSON 打印生成后的图片元数据；省略时返回原始图片字节。],
    "--width": [目标宽度（正整数像素）。`fit-width`、`scale`、`fit` 与 `fill` 必填。],
    "--height": [目标高度（正整数像素）。`fit-height`、`scale`、`fit` 与 `fill` 必填。],
    "--op": [缩放模式：`fit-width` / `fit-height`（按单边等比缩放）、`fit`（等比完整放入框内且不放大）、`fill`（裁剪填充至精确尺寸）、`scale`（直接拉伸）。省略时，仅给一边按比例缩放，两边都给则裁剪，均不给则保持原尺寸。],
    "--anchor": [`fill` 裁剪时的保留锚点，默认 `center`；也可选 `top`、`bottom`、`left`、`right`、`top-left`、`top-right`、`bottom-left` 或 `bottom-right`。其他模式仅接受 `center`。],
    "--format": [输出格式：`png`、`webp`、`jpg`、`jpeg` 或 `auto`。位图设为 `auto` 时，无透明通道的有损源图输出 JPEG，其余输出 PNG；SVG 必须显式指定位图格式。],
    "--quality": [编码质量：JPEG 取 `1..100`（默认 `75`），WebP 取 `0..100`（省略时无损），PNG 忽略该项。],
    "--filter": [位图重采样滤镜，默认 `lanczos3`；也可选 `nearest`、`triangle`、`catmull-rom` 或 `gaussian`。],
    "--background": [填充透明区域的六位十六进制底色（如 `'#ffffff'`）。含透明通道的图片转为 JPEG 时必填。],
  ),
)[
  SVG 引用的图片必须以内嵌 data URI 形式提供。站点 hook 渲染 SVG 文字时会自动沿用站点的 #ref("[typst.fonts]") 配置；独立运行时可在脚本名前传入 `--config tola.toml` 以加载站点字体。

  #example(title: [按宽度等比生成 WebP 预览图])[
    ```sh
    tola nu @tola-scripts/image resize photo.png --width 640 --format webp --output preview.webp
    ```
  ]

  #example(title: [顶部对齐裁剪方形缩略图])[
    ```sh
    tola nu @tola-scripts/image resize photo.png --width 160 --height 160 --anchor top --format webp --output square.webp
    ```
  ]

  #example(title: [使用站点字体光栅化 SVG])[
    ```sh
    tola nu --config tola.toml @tola-scripts/image resize wordmark.svg --width 240 --format png --output wordmark.png
    ```
  ]
]
