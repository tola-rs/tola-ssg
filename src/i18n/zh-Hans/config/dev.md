# `[dev]`

`tola dev` 的开发监听设置。

## 配置项

### `watch`

`tola dev` 是否监听保存的文件并刷新浏览器。

## 用法

从源码到能在浏览器里读的页面，共有三条命令。`tola build` 构建一次、写出发布目录后退出。`tola dev` 构建并把站点在本地提供出来，同时持续监听它读过的每个源：一次编辑会重建并刷新浏览器。`tola preview` 用生产设置构建一次，把那个结果在本地提供出来，但不监听，也不写 `publish-dir`，好让你在提交之前先看一遍即将上线的页面。

`watch` 属于 `tola dev`，三条命令里只有它会监听：

```toml
[dev]
watch = true
```

用三种方式跑同一个站点：

```sh
tola build      # write the publish directory and exit
tola dev        # serve locally and rebuild on every edit
tola preview    # serve the production build without watching
```

两个提供服务的命令共用的监听器是 `[server]`；`watch = false` 让 `tola dev` 只构建一次，但继续提供服务。

但它和 `tola preview` 仍不一样：`tola dev` 始终以 `dev` 模式构建（命令里能读到 `TOLA_BUILD_MODE=dev`），hook 按各自的 `dev` 字段参与——`after-publish` 设成 `dev = "run"` 时在这里也会跑；`tola preview` 用生产设置构建，从不运行 `after-publish`。
