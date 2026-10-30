# orca-profile-studio

[![CI](https://github.com/shixiaozhong/orca-profile-studio/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/shixiaozhong/orca-profile-studio/actions/workflows/ci.yml)

OrcaSlicer 系统配置文件（`resources/profiles`）的浏览、校验与编辑工具，也是一个 GPUI 练手项目。

> 这是个人维护的非官方工具，与 OrcaSlicer 项目、SoftFever 和 Snapmaker 均无隶属关系，也未获得它们的认可。仓库里没有从 OrcaSlicer 导出或复制的文件，工具只读取用户指定目录里的 profile 文件。本项目尚未选定 license。

目前完成了 M0 到 M17：
- M0：不依赖界面的 core 库，加一个命令行工具；
- M0.5：从 OrcaSlicer 导出的配置项 schema（已在 M16 中移除）；
- M1：用 gpui-kit 写的桌面浏览器；
- M2：继承链视图，列出预设每个键的最终值和来源层；
- M3：在界面里编辑预设的值，支持撤销和重做，保存时只改动变化的那几行；
- M4：并排对比同类型预设的最终值和来源，包含未保存的修改；
- M5：全局搜索和命令面板（⌘K）；
- M6：校验结果面板，以及兼容性条件表达式求值；
- M7：记住布局和上次的目录，亮色、暗色和跟随系统的主题，多窗口；
- M8：保存后自动刷新；Reload 保留未保存编辑、撤销历史和当前浏览位置；
- M9：按 Design Guides 审查界面，修正交互和错误反馈，关闭窗口和退出应用前保护未保存的编辑；
- M10：新建、复制、批量复制、重命名、删除预设，同步厂商清单；
- M11：批量设置或删除键、追加或移除字符串数组元素，逐预设预览，整组撤销和重做；
- M12：中英文界面和问题描述，以及保留编辑状态的语言切换；
- M13：机型的新建、复制、修改、重命名和删除，同步厂商清单与机器预设引用；
- M14：检测与继承值相同的覆盖值，将兄弟预设的共同值提到父项，预览所有受影响子孙；
- M15：GitHub Actions 持续集成，在 Linux 和 macOS 上跑格式、lint、全部测试和真实数据回归；
- M16：移除 schema，工具只分析 profile 文件本身；
- M17：打包成可双击启动的 macOS 应用，为 Windows 版做好快捷键、设置路径和清单路径的跨平台准备。

## 结构

```
crates/
├── profile-core/    # 加载、继承解析、校验；不依赖任何 GUI
├── profile-tool/    # 命令行：stats / check / resolve
└── profile-studio/  # gpui-kit 桌面应用；resources/icon.png 是应用图标
scripts/             # bundle-macos.sh 打包 macOS 应用，icon.swift 绘制图标
docs/roadmap.md      # 开发路线图
docs/reference/      # 配置格式说明（profile_format_spec.md）和参考统计脚本
AGENTS.md            # 给 Codex 的项目说明
CLAUDE.md            # 给 Claude Code 的项目说明，和 AGENTS.md 内容保持一致
```

## 桌面应用

开发时直接用 cargo 启动：

```bash
cargo run --release -p profile-studio -- ../OrcaSlicer/resources/profiles
```

也可以打包成 macOS 应用，之后双击启动：

```bash
scripts/bundle-macos.sh            # 生成 target/release/bundle/Orca Profile Studio.app
scripts/bundle-macos.sh --install  # 同时复制到 ~/Applications/
```

脚本做 release 构建，生成图标和 `Info.plist`，并做本地的 ad-hoc 签名（不做公证，不用于分发）。除了 `--install` 之外只写 `target/`。图标源文件是 `crates/profile-studio/resources/icon.png`，由 `swift scripts/icon.swift <输出路径>` 绘制。

默认启动时不打开任何目录，页面中间有「打开目录…」和「打开上次的目录」。以下情况会直接打开：
1. 命令行的第一个参数指定了目录；
2. 勾选了「启动时打开上次的目录」（起始页的勾选框，或 ⌘K 里的「启动时：打开上次的目录 / 不打开目录」）时，打开上次的目录；还没有打开过目录时，打开编译时和本仓库同级的 `../OrcaSlicer/resources/profiles`。

桌面应用不读取 `ORCA_PROFILES_DIR`（它只给命令行工具和测试用）。同一个启动设置也决定 ⌘N 新窗口是空白的还是打开上次的目录。从 Finder 或 Dock 启动时没有终端的环境变量，`ORCA_STUDIO_SETTINGS` 不生效。第 2 条里的同级检出用的是编译时的源码路径：从这个检出打包的 `.app` 能找到它，拷到别的机器上就找不到了。

设置文件在 macOS 上是 `~/Library/Application Support/orca-profile-studio/settings.json`，Windows 上是 `%APPDATA%\orca-profile-studio\settings.json`，其他平台是 `$XDG_CONFIG_HOME`（默认 `~/.config`）下的同名文件；`ORCA_STUDIO_SETTINGS` 可以指定别的路径。

系统通知（只在没有窗口能显示错误时使用）需要从 `.app` 启动：GPUI 只在有 Bundle ID 时启用 macOS 通知中心，`cargo run` 时只输出到 stderr。第一次发通知时系统会请求授权。

**选择要分析的目录**：工具栏标题旁的「打开目录…」（⌘O，File 菜单里也有）可以随时换成任意目录，比如另一个 OrcaSlicer 检出、某个分支或某个已安装版本的预设；旁边是当前目录的路径，悬停可看完整路径。没有打开目录或加载失败时，页面中间也有同一个按钮。选中的不是预设目录时，会依次查找其中的 `resources/profiles`（OrcaSlicer 源码检出或 Windows/Linux 安装目录）、`profiles`（`resources` 目录）和 `Contents/Resources/profiles`（macOS 的 `.app`）；都没有时提示「没有厂商配置」，不会加载出空树。判断标准是至少有一个与 `<厂商>` 文件夹并列的 `<厂商>.json`。命令行的 `--profiles` 用同样的规则。想并排看两个版本时，⌘N 打开新窗口后各自选择目录（默认新窗口是空白的）。

下文的快捷键按 macOS 写，其他平台用 Ctrl 代替 ⌘；工具栏提示会按当前平台显示。

界面说明：
- **左侧导航树**：厂商 → 机型（→ 该机型的打印机预设）、耗材、工艺、模板。树是虚拟化的，只渲染可见的行。有 warning 及以上级别问题的预设，行尾会显示警告图标。
- **右侧详情**：选中厂商、机型或预设后，显示文件信息、继承链、关键字段的最终值，以及该项的校验问题。
- **预设的 Values 页**：表格列出继承链上的文件写过的全部键，列有键名、最终值和来源层（本文件或第 n 层祖先）。整条链都没写的键不显示，它们的默认值在 OrcaSlicer 源码里，本工具不读取。
  - 可以按键名搜索，或只看本文件设置的键；点列头可以排序。
  - 选中一行，下方会列出继承链上设置过这个键的每一层和它的值，被覆盖的值显示为删除线，以及相关的问题（如与继承值相同的覆盖值）。
- **编辑**：在 Values 页选中一行就可以修改预设自己的值；继承来的值点「Override」后再修改，「Reset to inherited」删掉本文件的值、恢复继承。
  - 控件按值的形态选择：单个字符串用单行输入框，数组在多行文本框里一行一个元素，含换行的文本（如 G-code）原样编辑。值不按 OrcaSlicer 的类型校验。
  - 修改过的键和预设会标上铅笔图标；⌘Z / ⇧⌘Z 撤销和重做，⌘S 或「Save…」打开保存预览对话框，Enter 确认后才写盘，Escape 取消；仅保存已经 Apply 的编辑。
  - 保存时只替换变化的值，文件的缩进、数组写法、BOM 和末尾换行都保持原样；如果文件在加载后被外部改过，会拒绝保存。
  - 保存后在后台自动刷新，继承这个预设的其他预设（包括跨厂商继承公共耗材库的预设）、搜索、问题和兼容性一起更新。Reload 同样保留未保存编辑、撤销/重做历史、当前选中项、标签页和 Values 筛选。
  - 有编辑或历史记录的文件被外部改动、删除或无法加载时，会暂停数据替换，保留旧视图和所有编辑，并在窗口中提示。点「Review files…」查看具体文件；可以恢复磁盘文件后重试 Reload，也可以明确放弃这些文件的编辑和历史、采用磁盘版本，其他文件的编辑仍保留。Open 先选择目录，再明确确认是否丢弃当前编辑；取消选择或取消确认都保留内容。
- **预设对比**：在 Values 页点「Compare with…」，搜索并选择另一个同类型预设，支持跨厂商和模板。
  - 左右并排列出最终值和来源，默认只显示差异；关闭「Only differences」也能查看值相同、来源不同的键。
  - 默认按键名排列，可以按键名搜索，或点击列头排序图标排序；选中一行后，下方显示两边的继承层。一边没有写这个键时显示为「(missing)」。
  - 对比包含两边各自的编辑会话，未保存的修改在标题中标明；祖先值与 Values 页一致，使用最近一次加载的快照，保存祖先后自动刷新。刷新保留对比对象、筛选、排序和选中的键。
  - 按 `RawValue` 比较，字符串与数组、数组顺序、数值的不同字符串写法均计为差异；数组保留方括号显示，缺失值显示为 `(missing)`。
  - 对比页只读，点「Back to values」返回后保留原有的编辑、筛选和选择。
- **预设管理**（M10）：点工具栏的「Presets」，或在 ⌘K 里运行「Manage presets」。
  - 支持新建、复制、批量复制、重命名、删除预设。上方的表格按类型、厂商、搜索词和打印机型号筛选并勾选预设；筛选变化后，只保留仍显示的选中项。
  - 批量复制用一条名称替换规则（如 `0.4 nozzle` → `0.2 nozzle`）生成新的名字和文件名，组内的父项被复制时，新子项继承新父项；还可以对所有副本统一设置几个键（每行一个 `key = value`）。
  - 重命名会同时更新清单、`inherits`、各厂商的 `compatible_printers` / `compatible_prints`、默认工艺和耗材，以及机型的 `default_materials`；可选择把旧名记进 `renamed_from`。
  - 每次操作先预览全部文件改动和提示，确认后整批写入，失败会回滚。会新增 error 或 fatal 问题、删除还有子项的父项、或删除后会清空某个兼容列表时，阻止写入。有未保存的编辑时不能进入预设管理。
- **机型管理**（M13）：在预设管理顶部点「机型管理 / Machine models」，或通过 ⌘K 打开。
  - 从空白新建或复制现有机型，编辑名称、型号 ID、分号分隔的喷嘴规格与默认耗材，以及热床、贴图和热端资源路径。
  - 修改/重命名会同步厂商清单和同厂商的 `printer_model` 引用。仍有机器引用时禁止删除；移除正在使用的喷嘴规格也会阻止写入。资源文件只检查和提醒，不复制或删除。
  - 先预览，Enter 确认后整批写入，失败回滚。Reset input 清除未应用输入；返回浏览器、切换语言或 Reload 会保留输入，来源变化时需重置后重新预览。
  - 新建机型后，可继续用批量复制创建机器预设，再用批量编辑追加兼容列表。
- **批量编辑**（M11）：工具栏或 ⌘K 的「Batch edit」打开独立视图。
  - 按类型、厂商、搜索、机型、问题代码筛选，勾选同类型预设，选择键与操作：Set value、Remove key (inherit)、Add item、Remove item。可选的键是这一类预设实际写过的键。
  - 元素操作适用于数组值（如 `compatible_printers`）；保留每个预设自己列表中的其他元素，已有的元素不重复追加。继承来的列表需要修改时建立本地覆盖，预览会明确标出；同时选中的父项改完后子项已经继承到目标值时跳过，不写多余的覆盖值。Set value 的输入从第一个选中预设的当前值开始。
  - Preview 展示前后值、来源、新出现的多余覆盖值及受影响子孙数量；点预设名查看完整详情。Apply batch 只修改编辑会话，Save… 确认后才写盘。改变输入、筛选选择、编辑或 Reload 后须重新预览。
  - Undo batch / Redo batch 或 ⌘Z / ⇧⌘Z 整组操作；在 Values 页撤销属于批次的一步时也会整组撤销。成员有后续单独编辑时，先撤销这些后续编辑。
  - 返回浏览器后保留输入和编辑；Reset input 清除尚未应用的输入。保存沿用现有逐文件流程，部分失败会报告结果并刷新已保存的文件。
- **提到父项**（M14）：在批量编辑中选至少两个同父项、同类型的预设，选择键和「提到父项 / Promote to parent」。所选预设必须各自写有完全相同的值。
  - 预览列出父项、所选子项，以及最终值会变化的所有未选子孙。未选子孙的文件不会被改写，影响数量会在摘要中明确显示；跨厂商提升禁止。
  - 应用把值写到父项并删除子项覆盖，整批可以一步撤销/重做，保存后仍保留历史。父项已写相同值时只删子项覆盖。
  - `redundant-override` 是“与继承值相同”的 info 提示。在问题面板选“所有级别”并按此代码筛选即可查看；检测不自动修改任何配置。
- **问题面板**：点工具栏的「Issues」，或在 ⌘K 里运行「Show issues」，窗口底部会打开可调高度的问题列表。
  - 默认显示 warning 及以上；可以按最低严重级别、问题代码、厂商筛选，也可以按文字筛选。
  - 选中一行（点击或用 ↑↓）就会跳到对应的预设；问题涉及某个键时，会打开 Values 页并定位到这个键，焦点留在列表里。
- **兼容性**：预设的 Overview 里有「Compatibility」一节。耗材和工艺列出条件表达式（如果有）和兼容的打印机，打印机列出兼容的工艺和耗材数量。规则和 OrcaSlicer 一致：条件表达式、`compatible_printers` 列表、公共耗材库的排除机制，见规格文档第 6 节。
- **搜索和命令面板**：按 ⌘K 或点工具栏的「Search…」。
  - 可以搜预设（名称、别名、`setting_id`、`filament_id`）、配置项（键名和名称），以及文件里写的值；查询里的每个词都要出现，比如 `nozzle_temperature 270` 会找出把它设成 270 的文件。
  - 回车跳到选中的结果：预设会在树上展开并选中；值和配置项会打开 Values 页并筛选到这个键。
  - 也可以运行命令：重新加载、打开目录、切换 Overview / Values。
  - 只搜已加载的数据；保存后自动更新搜索索引，未保存的编辑不参与搜索。
- **语言**：默认跟随系统，⌘K 中搜索「Language」或「语言」可选择 English、中文或跟随系统，所有窗口同步切换并保存偏好。
  - 应用文案、问题描述和组件自带文字支持中英文；配置项始终显示键名，保存的值保持原样。
  - 切换语言保留未保存编辑、未应用文本、筛选与选择；批量预览需要重新生成。已显示的状态消息和底层技术错误详情可能保留原文。
  - 应用文案的中英对照在 [`locales/app.yml`](crates/profile-studio/locales/app.yml)，不需要修改代码即可调整译文。
- **主题和窗口**：在 ⌘K 里运行「Theme: System / Light / Dark」切换主题，默认跟随系统。⌘N 或 File 菜单的「New Window」打开一个独立的新窗口；是否加载上次的目录跟随启动设置。
- **关闭和退出**：原生关闭或 ⌘W 检查当前窗口，应用菜单 Quit（其他平台是 File 菜单的 Exit）或 ⌘Q 检查全部窗口；有未保存编辑或尚未 Apply 的输入时先确认，取消后继续编辑。确认期间其他窗口又新增未保存文件时会重新提示。系统直接终止/强制退出受 GPUI 平台 API 限制，无法取消。
- **错误反馈**：设置读写、目录选择、打开新窗口失败时在窗口中显示通知，提供 Dismiss；设置保存失败不影响当前会话，恢复权限后可重试相同设置。
- **记住的设置**：主题、语言、上次的目录、启动时是否打开它、侧栏宽度、问题面板的开关和高度、窗口大小，保存在上文所列的设置文件里。
- **后台加载**：加载在后台线程进行，刷新期间保留可操作的旧视图，完成后一次性替换。期间新增的编辑会一起迁移；新的加载请求会取代旧请求，刷新期间再次保存也会重新加载。刷新失败会保留旧视图和编辑，并提示重试。

界面测试在 `crates/profile-studio/src/app.rs` 的 `tests` 模块里，用的是 gpui-kit 的 headless 测试：渲染真实界面，模拟点击，然后断言状态。

`profile-core` 按 OrcaSlicer `PresetBundle::load_vendor_configs_from_json` 的规则加载配置，规则详见 [docs/reference/profile_format_spec.md](docs/reference/profile_format_spec.md)：

- 每个厂商单独加载，每一类（process / filament / machine）有自己的名字表，严格按清单顺序处理；
- 子配置整键覆盖父配置，数组整个替换，不按元素合并；
- 唯一的跨厂商继承来源是 OrcaFilamentLibrary 的耗材；
- 读文件时容忍 UTF-8 BOM（OrcaSlicer 用的 nlohmann::json 会跳过 BOM，`serde_json` 不会）。

和 OrcaSlicer 的区别：**加载过程从不中断**。所有偏差都会记成 `Issue`，按严重程度分为四级：
- `fatal`：OrcaSlicer 会因此拒绝加载整个厂商；
- `error`：OrcaSlicer 会记错误日志，但继续加载；
- `warning`：能加载，但很可能不是作者的本意；
- `info`：仅供参考。

### Windows

在 Windows 上用 MSVC 工具链编译（不支持从 Mac 交叉编译）：

```powershell
cargo run --release -p profile-studio -- D:\OrcaSlicer\resources\profiles
powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
```

打包脚本生成 `target\release\package\orca-profile-studio-<版本>-windows-<x64|arm64>.zip`，里面是带图标和版本信息的 `Orca Profile Studio.exe` 和一份使用说明。解压后双击 exe 即可，不需要安装。release 版不弹出控制台窗口。exe 没有签名，第一次运行时 SmartScreen 可能拦截，点「更多信息」→「仍要运行」。

- Windows 版没有菜单栏：新窗口、关闭、退出用 Ctrl+N / Ctrl+W / Ctrl+Q，其余操作在工具栏和 Ctrl+K 里；关掉最后一个窗口时退出。
- 路径显示为普通的 `D:\…` 形式（不是 `\\?\D:\…`）。清单里的 `sub_path` 写回时一律用 `/`。
- 保存保留文件原有的换行（CRLF 或 LF）和 BOM，所以 Git 的 `core.autocrlf` 设成什么都不会产生额外差异。
- 预设管理优先用硬链接原子地安装文件；在不支持硬链接的盘（FAT/exFAT U 盘、部分网络盘）上改为新建文件再写入，同样不会覆盖已有文件。
- `.ico` 由 `scripts/icon-windows.sh`（在 Mac 上运行）从同一张图标 PNG 生成。

## 使用

需要 Rust 1.88 及以上版本。

```bash
export ORCA_PROFILES_DIR=../OrcaSlicer/resources/profiles
```

```bash
cargo run --release -p profile-tool -- stats
```

```bash
cargo run --release -p profile-tool -- check --vendor Snapmaker --min info
```

```bash
cargo run --release -p profile-tool -- resolve "Generic ABS @U1 0.2 nozzle" --key temperature
```

`check` 默认在出现 `error` 及以上级别的问题时返回退出码 1（用 `--fail-on` 调整），可以直接放进 CI。

```bash
cargo run --release -p profile-tool -- search "nozzle_temperature 270"
```

`search` 和桌面应用的 ⌘K 用同一套搜索，列出匹配的预设、配置项和文件里写的值。

```bash
cargo run --release -p profile-tool -- compat "0.07mm DETAIL @CORE One 0.25"
```

`compat` 列出一个耗材或工艺兼容的打印机，以及每台兼容的原因（条件、列表或没有限制）；对打印机则列出它兼容的工艺和耗材。

`compat-check <表>` 拿 OrcaSlicer 导出的兼容性表（`Snapmaker_Orca_compat_export`，命令见 `AGENTS.md`）逐项对照。条件里的变量按值的文本推断类型（整数、数字或字符串）；在 OrcaSlicer `953509c23f` 的数据上，约 400 万个（预设，打印机）组合全部一致。

`resolve` 列出继承链上的文件设置过的值。

## 不使用 schema

配置文件里只有字符串，各配置项的类型、默认值、范围、枚举值和界面文字都定义在 OrcaSlicer 的 C++ 代码里（`print_config_def`）。M0.5 曾把这份定义导出成 `schema.json` 内嵌进来，用来校验参数值、补全默认值；M16 起整个项目不再使用它，避免和某个 OrcaSlicer 版本强绑定。因此本工具不报告未知键、类型或取值范围这类问题，只做结构检查（清单、继承、重名、编码、兼容性条件等）和多余覆盖值检测。严重程度按 OrcaSlicer 实际的加载行为来定，见 `crates/profile-core/src/issue.rs`。

## 测试

需要 Rust 1.95 或更新版本（GPUI 使用了 1.95 才稳定的 `std::hint::cold_path`）。

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

`crates/profile-core/tests/orca_profiles.rs` 是针对真实数据的回归测试。它会读取 `ORCA_PROFILES_DIR` 指定的目录，没有设置时使用和本仓库同级的 `../OrcaSlicer` 目录；两者都找不到就跳过。测试里的期望数字对应 OrcaSlicer `953509c23f` 的配置数据，数据变化后需要同步更新。`ORCA_PROFILES_DIR` 写相对路径时，按仓库根目录解析。

### 持续集成

[CI workflow](.github/workflows/ci.yml) 在推送到 `main`、提交 PR 和手动触发时运行：

- **Linux（Ubuntu 24.04）**：格式检查，`profile-core` / `profile-tool` 的 clippy 和测试。
- **macOS 15**：整个 workspace 的 clippy 和测试，包括 headless 界面测试。
- **Windows（Server 2025，MSVC）**：整个 workspace 的 clippy 和测试，包括界面测试；保留 Git 默认的换行转换，profiles 按 CRLF 检出；最后用打包脚本生成 zip（不上传）并核对 exe 的版本信息。
- **最低 Rust 版本**：三个平台都从 `Cargo.toml` 读取 `rust-version`，用该版本检查对应包的全部 targets。
- **真实数据回归**：三个平台的测试任务都从公开 OrcaSlicer fork 稀疏检出锁定提交的 profiles，先检查数据存在及提交正确，再设置 `ORCA_PROFILES_DIR` 运行测试，避免缺数据时静默跳过。
- **独立统计核对**：Python 参考脚本检查固定统计数字；不一致时退出码为 1。

所有构建、lint 和测试均使用 `--locked`；Action 固定到提交 SHA，按系统和工具链缓存依赖，锁定的 OrcaSlicer 数据按 `ORCA_COMMIT` 缓存。只取消 PR 上被新推送取代的运行，`main` 上的每个提交都有自己的结果。CI 只读仓库，不推送或发布产物。数据提交集中在 workflow 的 `ORCA_COMMIT`，升级数据时也要更新 Rust 测试和 Python 的 `--check` 基线。

本地运行同样的 Python 检查：

```bash
python3 -I docs/reference/profile_format_stats.py ../OrcaSlicer/resources/profiles --check
```

## 路线图

已完成 M0 到 M16。接下来是 M17 打包成 macOS 应用并做好跨平台准备、M18 Windows 版；和 Git 对比暂时待定。各阶段的范围、关键设计决定、依赖和完成标准见 [docs/roadmap.md](docs/roadmap.md)。
