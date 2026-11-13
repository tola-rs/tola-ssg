# `@tola-scripts/image`

查看图片信息，或另存一张不同尺寸或格式的图片。

## `main`

用 `metadata` 查看图片信息，用 `resize` 另存图片。也可直接运行 `tola nu @tola-scripts/image` 查看子命令。

## `metadata`

输出 JSON，包括图片的像素宽高（`width`、`height`）、格式（`format`）和是否支持透明度（`has-alpha`）。

示例：查看 PNG 图片信息：

把一张 PNG 图片保存为当前目录下的 `photo.png`，然后运行：

```sh
tola nu @tola-scripts/image metadata photo.png
```

### 参数

- `source`：图片文件路径。相对路径以命令运行目录为准。

## `resize`

缩放或转换本地图片，用新的文件名保存结果。转换 SVG 时，要指定 PNG、WebP 等输出格式。SVG 引用的图片要用 data URI 内嵌。

示例：生成 WebP 预览图：

在 `photo.png` 所在目录运行：

```sh
tola nu @tola-scripts/image resize photo.png --width 640 --format webp --output preview.webp
```

命令会生成宽 640 像素的 `preview.webp`，高度按原图宽高比计算。

示例：裁出方形缩略图：

要裁成正方形，传入宽和高。加上 `--anchor top` 可以保留照片顶部：

```sh
tola nu @tola-scripts/image resize photo.png --width 160 --height 160 --anchor top --format webp --output square.webp
```

命令会在同一目录生成 160 × 160 像素的 `square.webp`。

示例：把 SVG 转成 PNG：

将下面的代码保存为当前目录下的 `favicon.svg`：

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <circle cx="16" cy="16" r="14" fill="#3569b0"/>
</svg>
```

在同一目录运行：

```sh
tola nu @tola-scripts/image resize favicon.svg --width 64 --format png --output favicon-64.png
```

生成一张 64 × 64 像素的 PNG。

示例：用站点字体绘制 SVG 文字：

把带文字的 `wordmark.svg` 和 `tola.toml` 放在一起。用 `tola inspect fonts` 查字体族名称，将名称填进 SVG 的 `font-family`。站点 hook 会自动使用站点的 `[typst.fonts]`。单独运行图片脚本时，要用站点字体就在脚本名之前传入配置文件：

```sh
tola nu --config tola.toml @tola-scripts/image resize wordmark.svg --width 240 --format png --output wordmark.png
```

命令会生成宽 240 像素的 `wordmark.png`，高度按原图比例计算。开发模式复用 hook 产物时，把脚本读取的 SVG 和站点字体文件加到 `rerun-on`。如果结果依赖系统字体，使用 `dev = "run"`。

### 参数

- `source`：图片文件路径，支持 SVG。
- `--output`：把图片保存到已有目录下的新文件，并输出 JSON 图片信息。省略时返回图片字节。
- `--width`：目标宽度，单位是像素，必须为正整数。`fit-width`、`scale`、`fit` 和 `fill` 必填。`fit-height` 按比例推算宽度。
- `--height`：目标高度，单位是像素，必须为正整数。`fit-height`、`scale`、`fit` 和 `fill` 必填。`fit-width` 按比例推算高度。
- `--op`：用 `fit-width` 或 `fit-height` 指定一边并保持宽高比。用 `fit` 把整张图放进指定尺寸内且不放大，用 `fill` 裁到指定尺寸，用 `scale` 直接拉伸到指定尺寸。省略时，只给一边就按比例缩放，两边都给就裁剪，宽和高都没给则保留原尺寸。
- `--anchor`：`fill` 裁剪时保留的位置，默认居中（`center`）。要保留边缘或角落，可用 `top`、`bottom`、`left`、`right`、`top-left`、`top-right`、`bottom-left` 或 `bottom-right`。其他操作只接受 `center`。
- `--format`：可用 `png`、`webp`、`jpg` 或 `jpeg`。位图输入设为 `auto` 时，有损且无 alpha 的源图转成 JPEG，其余转成 PNG。SVG 必须显式指定格式。请使用与输出格式相符的文件扩展名。
- `--quality`：JPEG 为 1–100，省略取 75。WebP 为 0–100，省略时无损。PNG 忽略此项。
- `--filter`：位图的重采样滤镜。省略时用 `lanczos3`。也可选 `nearest`、`triangle`、`catmull-rom` 或 `gaussian`。
- `--background`：填充透明区域的底色。比如 `--background '#ffffff'` 用白色填充，`#` 后须有六位十六进制数字。有 alpha 的图片转 JPEG 时必填。
