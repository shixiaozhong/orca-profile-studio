# orca-profile-studio

OrcaSlicer 系统配置文件（`resources/profiles`）的浏览、校验与编辑工具，也是一个 GPUI 练手项目。

目前完成了 M0 到 M8：
- M0：不依赖界面的 core 库，加一个命令行工具；
- M0.5：从 OrcaSlicer 导出的配置项 schema，用来校验参数值、补全默认值；
- M1：用 gpui-kit 写的桌面浏览器；
- M2：继承链视图，列出预设每个键的最终值和来源层；
- M3：在界面里编辑预设的值，支持撤销和重做，保存时只改动变化的那几行；
- M4：并排对比同类型预设的最终值和来源，包含未保存的修改；
- M5：全局搜索和命令面板（⌘K）；
- M6：校验结果面板，以及兼容性条件表达式求值；
- M7：记住布局和上次的目录，亮色、暗色和跟随系统的主题，多窗口。
- M8：保存后自动刷新；Reload 保留未保存编辑、撤销历史和当前浏览位置。

## 结构

```
crates/
├── profile-core/    # 加载、继承解析、校验；不依赖任何 GUI
│   └── schema/      # 从 OrcaSlicer 导出的 schema.json，编译时内嵌
├── profile-tool/    # 命令行：stats / check / resolve
└── profile-studio/  # gpui-kit 桌面应用
docs/roadmap.md      # 开发路线图
docs/reference/      # 配置格式说明（profile_format_spec.md）和参考统计脚本
AGENTS.md            # 给 Codex 的项目说明
CLAUDE.md            # 给 Claude Code 的项目说明，和 AGENTS.md 内容保持一致
```

## 桌面应用

```bash
cargo run -p profile-studio -- ../OrcaSlicer/resources/profiles
```

profiles 目录按以下顺序确定：
1. 命令行第一个参数；
2. 环境变量 `ORCA_PROFILES_DIR`；
3. 和本仓库同级的 `../OrcaSlicer` 目录。

运行中也可以点工具栏的「Open…」换一个目录。

界面说明：
- **左侧导航树**：厂商 → 机型（→ 该机型的打印机预设）、耗材、工艺、模板。树是虚拟化的，只渲染可见的行。有 warning 及以上级别问题的预设，行尾会显示警告图标。
- **右侧详情**：选中厂商、机型或预设后，显示文件信息、继承链、关键字段的最终值，以及该项的校验问题。
- **预设的 Values 页**：表格列出预设的全部键，列有类别、配置项名称、键名、最终值（带单位）和来源层（本文件、第 n 层祖先或 schema 默认值）。
  - 可以按键名、名称或类别搜索，可以隐藏默认值，或只看本文件设置的键；点列头可以排序。
  - 选中一行，下方会列出继承链上设置过这个键的每一层和它的值，被覆盖的值显示为删除线。同时显示配置项说明、类型、取值范围和相关的校验问题。
- **编辑**：在 Values 页选中一行就可以修改预设自己的值；继承来的值点「Override」后再修改，「Reset to inherited」删掉本文件的值、恢复继承。
  - 控件按类型选择：bool 是复选框，枚举是下拉框，数组在多行文本框里一行一个元素，G-code 原样编辑。
  - 修改过的键和预设会标上铅笔图标；⌘Z / ⇧⌘Z 撤销和重做，⌘S 或「Save…」先列出所有改动，确认后才写盘。
  - 保存时只替换变化的值，文件的缩进、数组写法、BOM 和末尾换行都保持原样；如果文件在加载后被外部改过，会拒绝保存。
  - 保存后在后台自动刷新，继承这个预设的其他预设（包括跨厂商继承公共耗材库的预设）、搜索、问题和兼容性一起更新。Reload 同样保留未保存编辑、撤销/重做历史、当前选中项、标签页和 Values 筛选。
  - 有编辑或历史记录的文件被外部改动、删除或无法加载时，会暂停数据替换，保留旧视图和所有编辑，并在窗口中提示。点「Review files…」查看具体文件；可以恢复磁盘文件后重试 Reload，也可以明确放弃这些文件的编辑和历史、采用磁盘版本，其他文件的编辑仍保留。Open 切换目录仍需确认丢弃未保存编辑。
- **预设对比**：在 Values 页点「Compare with…」，搜索并选择另一个同类型预设，支持跨厂商和模板。
  - 左右并排列出最终值和来源，默认只显示差异；关闭「Only differences」也能查看值相同、来源不同的键。
  - 默认按类别和名称排列，可以按键名、名称、类别搜索，或点击列头排序图标排序；选中一行后，下方显示两边的继承层。
  - 对比包含两边各自的编辑会话，未保存的修改在标题中标明；祖先值与 Values 页一致，使用最近一次加载的快照，保存祖先后自动刷新。刷新保留对比对象、筛选、排序和选中的键。
  - 按 `RawValue` 比较，字符串与数组、数组顺序、数值的不同字符串写法均计为差异；数组保留方括号显示，缺失值显示为 `(missing)`。
  - 对比页只读，点「Back to values」返回后保留原有的编辑、筛选和选择。
- **问题面板**：点工具栏的「Issues」，或在 ⌘K 里运行「Show issues」，窗口底部会打开可调高度的问题列表。
  - 默认显示 warning 及以上；可以按最低严重级别、问题代码、厂商筛选，也可以按文字筛选。
  - 选中一行（点击或用 ↑↓）就会跳到对应的预设；问题涉及某个键时，会打开 Values 页并定位到这个键，焦点留在列表里。
- **兼容性**：预设的 Overview 里有「Compatibility」一节。耗材和工艺列出条件表达式（如果有）和兼容的打印机，打印机列出兼容的工艺和耗材数量。规则和 OrcaSlicer 一致：条件表达式、`compatible_printers` 列表、公共耗材库的排除机制，见规格文档第 6 节。
- **搜索和命令面板**：按 ⌘K 或点工具栏的「Search…」。
  - 可以搜预设（名称、别名、`setting_id`、`filament_id`）、配置项（键名和名称），以及文件里写的值；查询里的每个词都要出现，比如 `nozzle_temperature 270` 会找出把它设成 270 的文件。
  - 回车跳到选中的结果：预设会在树上展开并选中；值和配置项会打开 Values 页并筛选到这个键。
  - 也可以运行命令：重新加载、打开目录、切换 Overview / Values。
  - 只搜已加载的数据；保存后自动更新搜索索引，未保存的编辑不参与搜索。
- **主题和窗口**：在 ⌘K 里运行「Theme: System / Light / Dark」切换主题，默认跟随系统。⌘N 或 File 菜单的「New Window」打开一个独立的新窗口，加载上次的目录。
- **记住的设置**：主题、上次的目录、侧栏宽度、问题面板的开关和高度、窗口大小，保存在 `~/Library/Application Support/orca-profile-studio/settings.json`（可用 `ORCA_STUDIO_SETTINGS` 指定别的路径）。不带参数启动时，打开上次的目录。
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

`compat-check <表>` 拿 OrcaSlicer 导出的兼容性表（`Snapmaker_Orca_compat_export`，命令见 `AGENTS.md`）逐项对照。在 OrcaSlicer `953509c23f` 的数据上，约 400 万个（预设，打印机）组合全部一致。

`resolve` 默认只列出继承链上的文件设置过的值；加 `--defaults` 会把来自 schema 默认值的键也列出来（标记为 `[d]`）。

## Schema

配置文件里只有字符串，各配置项的类型、默认值、范围、枚举值和界面文字都定义在 OrcaSlicer 的 C++ 代码里（`print_config_def`）。`crates/profile-core/schema/schema.json` 是从 OrcaSlicer 导出的这份定义，内容包括：
- 每个配置项的类型、默认值、范围、枚举值、label、tooltip、类别和模式；
- 三类默认预设的完整内容，也就是没有 `inherits` 的预设的起点；
- `Preset::print_options()` 等键列表。

命令行和桌面应用默认使用内嵌的 schema：
- `--schema <文件>` 换成别的 schema；
- `--no-schema` 不使用 schema。

使用 schema 时会多做这些检查：
- 未知键、过时键、改了名的旧键；
- 不属于该类型的键；
- 数值解析失败或后面带多余文字；
- 枚举值和布尔值不合法；
- 标量写成了数组，非 nullable 的数组里出现 `nil`；
- 超出界面上的取值范围。

严重程度按 OrcaSlicer 实际的加载行为来定，见 `crates/profile-core/src/issue.rs`。

导出程序是 OrcaSlicer 仓库里的 `src/dev-utils/Snapmaker_Orca_schema_export.cpp`（`personal/schema-export` 分支）。OrcaSlicer 更新后按 `AGENTS.md` 里的命令重新生成。`src/legacy.rs` 是 `PrintConfigDef::handle_legacy` 的手工移植，重新生成 schema 时也要对照更新。

## 测试

```bash
cargo test
```

`crates/profile-core/tests/orca_profiles.rs` 是针对真实数据的回归测试。它会读取 `ORCA_PROFILES_DIR` 指定的目录，没有设置时使用和本仓库同级的 `../OrcaSlicer` 目录；两者都找不到就跳过。测试里的期望数字对应 OrcaSlicer `953509c23f` 的配置数据，数据变化后需要同步更新。`ORCA_PROFILES_DIR` 写相对路径时，按仓库根目录解析。

## 路线图

已完成 M0 到 M8，接下来是 M9 到 M13：按 Design Guides 审查早期界面、批量编辑、新建和复制预设、中文界面、和 Git 对比。各阶段的范围、关键设计决定、依赖和完成标准见 [docs/roadmap.md](docs/roadmap.md)。
