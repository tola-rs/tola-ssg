# `[options]`

站点模板、Nu 脚本和站点 hooks 共享的选项。

## 用法

在顶层 `[options]` 表中声明站点数据。Typst 通过 `site.options` 读取，Nu 通过 `(get-site).options` 读取。站点 hook 的 `enabled-by` 引用这张表的顶层布尔键。TOML 日期和时间转换为字符串，数字必须是有限值。模块实例的选项独立配置在 `[modules.<alias>.options]`。
