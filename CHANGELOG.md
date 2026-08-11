# 更新日志

版本号遵循 `主版本.次版本.修订号`。安装包由 `v*` tag 触发 CI 构建产出，见 [Releases](https://github.com/dream-rec/dreampaper/releases)。

## v0.1.1

以修 Windows 端的问题为主，macOS 端行为不变。

### 修复

- **Windows 模版库、导入弹窗的图片预览空白**。自定义协议的 URL 形式两个平台不一样：macOS 是 `<scheme>://localhost/<path>`，而 Windows 的 WebView2 注册不了自定义协议，只能走 `http://<scheme>.localhost/<path>`。原先只按 macOS 的形式拼 URL，Windows 上自然取不到图。
- **Windows 启动时多弹一个终端窗口，且关掉它主窗口跟着退出**。发布构建缺 `windows_subsystem = "windows"`，控制台成了进程的父窗口。
- **Windows 安装版窗口不是圆角**。改为在 Win11 上通过 DWM 显式请求圆角。Win10 没有这个系统能力，保持直角，功能不受影响。
- **模版库导入弹窗里的 PaperBench 下载链接点击无反应**。Tauri 的 webview 没有 `window.open` 处理器，`<a target="_blank">` 既不跳转也不报错，URL 必须交给系统浏览器打开。
- **结果图预览点击无反应**（未报告，本次一并修）。与上一条同一个根因，改为调用系统默认看图程序打开。
- **Windows 窗口顶部约 48px 空白**（未报告，本次一并修）。`titleBarStyle: "Overlay"` 只在 macOS 生效，Windows 保留原生标题栏，为红绿灯按钮预留的位置就成了死区。

### 其他

- 项目改用 [PolyForm Noncommercial License 1.0.0](LICENSE)：允许非商业使用，禁止商业使用。
- 更换 README 里的 slide 示例截图。
- 清理仓库内约 900 行中文代码注释，注释统一改用英文。界面文案、错误提示、测试断言消息与发布说明仍为中文。

## v0.1.0

首个公开版本。
