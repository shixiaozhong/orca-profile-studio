Orca Profile Studio（Windows 免安装版）
=======================================

用来浏览、校验和编辑 OrcaSlicer 系统配置文件（resources\profiles）的个人工具，
与 OrcaSlicer、SoftFever、Snapmaker 均无隶属关系。

启动
----
1. 把 zip 解压到任意目录，双击“Orca Profile Studio.exe”。
2. 启动后点“打开目录…”（Ctrl+O），选择要分析的预设目录：OrcaSlicer 的 resources\profiles，
   或直接选 OrcaSlicer 的源码检出、安装目录，会自动找到里面的 profiles。
   起始页勾选“启动时打开上次的目录”后，下次启动直接打开上次的目录。
3. 也可以在命令行里把目录作为第一个参数传入：
   "Orca Profile Studio.exe" D:\OrcaSlicer\resources\profiles

程序没有签名，第一次运行时 SmartScreen 可能拦截：点“更多信息”→“仍要运行”。

注意
----
- 保存会直接写进打开的 profiles 目录。建议在 Git 检出里使用，随时可以用 git 查看或还原。
- 设置（主题、语言、上次的目录、窗口大小）保存在
  %APPDATA%\orca-profile-studio\settings.json
- 快捷键：Ctrl+O 打开目录，Ctrl+K 搜索和命令面板，Ctrl+S 保存，Ctrl+Z / Ctrl+Shift+Z 撤销和重做，
  Ctrl+N 新窗口，Ctrl+W 关闭窗口，Ctrl+Q 退出（关掉最后一个窗口也会退出）。
  Windows 版没有菜单栏，其余操作都在工具栏和 Ctrl+K 里。
- 界面语言默认跟随系统，可以在 Ctrl+K 里搜索“语言”切换。

Orca Profile Studio (portable Windows build)
--------------------------------------------
Unzip anywhere and run "Orca Profile Studio.exe". Use "Open folder…" (Ctrl+O) to choose
OrcaSlicer's resources\profiles folder, or an OrcaSlicer checkout or installation that contains it.
Saving writes into that folder.
Settings: %APPDATA%\orca-profile-studio\settings.json
