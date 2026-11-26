#show: config(
  summary: [将站点配置拆分到多份本地 TOML 文件中按顺序合并。],
  related: ("[[hooks.before-build]]", "[assets]", "[modules]"),
)

将 `include` 写在 `tola.toml` 顶部、第一个 `[表头]` 之前。数组元素可以是路径字符串，也可以是 `{ path = "...", optional = true }`。

= 加载顺序与路径规则

- `include` 中的相对路径以声明它的文件所在目录为基准；被引入文件内的站点路径配置（如 `build.publish-dir` 或本地模块的 `path`）仍以站点根目录为基准。
- 每一项都必须是字面本地路径，不支持通配符、环境变量展开或远程 URL。绝对路径受当前输入范围约束（例如 `--pure` 要求配置文件位于站点内）。
- 省略 `optional` 时文件必须存在；设为 `true` 时允许缺失，但文件存在时仍需通过权限、输入范围和格式校验。
- 按数组顺序深度优先合并：先处理被引入文件自身的 `include`，再合并该文件本身；`tola.toml` 最后合并，缺失的键最后由默认值补齐。
- 同一文件被多次引入时会重复生效（同时重复追加其中的 hook 与资源，可能引发重名或路径冲突）；循环引入会直接报错。

= 合并规则

表按字段逐键合并；标量与普通数组由后加载的值覆盖。资源与 hook 数组（`assets.files`、`assets.trees`、`hooks.before-build`、`hooks.generate-outputs`、`hooks.after-publish`）按加载顺序追加。

#table(
  columns: 2,
  table.header([后加载的写法], [合并结果]),
  [省略某个键], [保留先前的配置值。],
  [`title = ""`], [替换为空字符串（前提是该字段允许空字符串）。],
  [`[site]` 中写 `languages = []`], [将先前的语言列表替换为空数组。],
  [`[assets]` 中写 `files = []`], [追加零条记录，先前声明保留。],
  [空表], [保留该表内已有的全部键。],
)

运行 `tola inspect config --json --pretty`，可通过 `config_sources` 查看加载顺序与 `optional-missing` 状态，通过 `field_origins` 查看每个配置键的最终来源与覆盖关系。

#example(title: [拆分公共基础配置与可选的本地覆盖])[
  ```toml
  include = [
    "config/base.toml",
    { path = "config/local.toml", optional = true },
  ]
  ```
]
