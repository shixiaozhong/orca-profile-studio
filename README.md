# orca-profile-studio

OrcaSlicer 系统配置文件（`resources/profiles`）的浏览、校验与编辑工具，也是一个 GPUI 练手项目。

目前完成了 M0 和 M1：
- M0：不依赖界面的 core 库，加一个命令行工具；
- M1：用 gpui-kit 写的桌面浏览器。

## 结构

```
crates/
├── profile-core/    # 加载、继承解析、校验；不依赖任何 GUI
├── profile-tool/    # 命令行：stats / check / resolve
└── profile-studio/  # gpui-kit 桌面应用
docs/reference/      # 配置格式说明（profile_format_spec.md）和参考统计脚本
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
- **后台加载**：加载在后台线程进行，界面期间保持可响应。重新加载或打开新目录时，会丢弃旧的加载结果。

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

## 测试

```bash
cargo test
```

`crates/profile-core/tests/orca_profiles.rs` 是针对真实数据的回归测试。它会读取 `ORCA_PROFILES_DIR` 指定的目录，没有设置时使用和本仓库同级的 `../OrcaSlicer` 目录；两者都找不到就跳过。测试里的期望数字对应 OrcaSlicer `953509c23f` 的配置数据，数据变化后需要同步更新。

## 路线图

| 阶段 | 内容 |
|---|---|
| M0 ✅ | core：清单解析、继承解析、校验；CLI：stats / check / resolve |
| M0.5 | 从 OrcaSlicer 导出 `schema.json`（各配置项的类型、默认值、范围、说明、所属类别） |
| M1 ✅ | gpui-kit 界面：厂商 → 机型、耗材、工艺三层树，后台加载 |
| M2 | 继承链视图：每个字段的最终值和来源层 |
| M3 | 编辑表单：修改标记、撤销和重做、保存时保留原有格式 |
| M4 | 两个预设并排对比 |
| M5 | 全局搜索和命令面板 |
| M6 | 校验结果面板，以及兼容性条件表达式求值 |
| M7 | 可停靠布局、主题、多窗口 |
| 扩展 | 2D G-code 分层查看器 |
