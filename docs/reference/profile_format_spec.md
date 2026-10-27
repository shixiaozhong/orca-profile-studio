# Orca 系统配置文件格式说明

> 用途：为独立的「配置文件工作台」项目（Rust + gpui-kit）提供数据格式与加载语义的参考。
> 依据：OrcaSlicer 仓库 `personal/notes` 分支（基于 `953509c23f`）的 `resources/profiles/` 数据与 `src/libslic3r` 加载代码，整理于 2026-10-25。
> 本文复制自 OrcaSlicer 仓库的 `docs/`。文中所有源码路径都相对于 OrcaSlicer 仓库根目录。
> 配套脚本：[profile_format_stats.py](profile_format_stats.py) 按本文描述的规则模拟加载全部数据并输出统计，文中的数字均出自它。
> `profile-core` 已复现这些数字，见 `crates/profile-core/tests/orca_profiles.rs`。只有一处不同：`compatible_printers` 的引用总数。本文统计的是文件里直接写的值（13459），`profile-core` 统计的是沿继承链解析后的值（14082）；两者得出的失效引用数相同，都是 581。

```bash
python3 -I docs/reference/profile_format_stats.py ../OrcaSlicer/resources/profiles
```

---

## 1. 数据规模

| 项 | 数量 |
|---|---|
| 厂商（含 OrcaFilamentLibrary） | 57 |
| 机型文件 `machine_model` | 326 |
| 工艺 `process`（可选 / 模板） | 2523（2156 / 367） |
| 耗材 `filament`（可选 / 模板） | 3637（2916 / 721） |
| 打印机 `machine`（可选 / 模板） | 935（796 / 139） |
| 每类出现过的不同参数键 | process 333、filament 180、machine 162 |
| 参数值形态 | 字符串 113725、单元素数组 74880、多元素数组 8861 |

可选预设（`instantiation: "true"`）的继承深度分布：

| 深度 | 0 | 1 | 2 | 3 | 4 | 5 | 6 |
|---|---|---|---|---|---|---|---|
| 数量 | 31 | 585 | 1843 | 2725 | 623 | 56 | 5 |

深度 3 最常见，最深 6 层。界面上的「继承链」视图至少要能舒服地展示 6 层以上。

---

## 2. 目录结构

```
resources/profiles/
├── <Vendor>.json                 # 厂商索引（清单）
├── <Vendor>/
│   ├── machine/                  # machine_model 与 machine 两类文件都在这里
│   ├── process/
│   ├── filament/                 # 可有任意子目录，如 filament/Polymaker/
│   ├── <Model>_cover.png         # 机型封面图
│   ├── <Model>_bed.stl           # 热床模型
│   └── <Model>_texture.svg       # 热床贴图
├── OrcaFilamentLibrary.json      # 公共耗材库（清单为空，见 3.2）
├── OrcaFilamentLibrary/filament/{base/, *.json, <Brand>/}
├── blacklist.json                # 不是厂商，见 7
└── check_unused_setting_id.py
```

另有 `resources/profiles_template/Template.json` + `Template/`，作为新建自定义配置的模板厂商，加载规则上有特例（见 5.3）。

**加载只认清单，不扫描目录**（OrcaFilamentLibrary 除外）。目录里存在但清单没列出的 JSON 不会被加载，见 8.4。

---

## 3. 厂商索引文件 `<Vendor>.json`

### 3.1 结构

```json
{
  "name": "Snapmaker",
  "version": "02.03.04.04",
  "force_update": "0",
  "description": "Snapmaker configurations",
  "url": "...",                                // 可选，13 个厂商有
  "machine_model_list": [ { "name": "Snapmaker U1", "sub_path": "machine/Snapmaker U1.json" } ],
  "process_list":       [ { "name": "...", "sub_path": "process/....json" } ],
  "filament_list":      [ ... ],
  "machine_list":       [ ... ]
}
```

| 字段 | 说明 |
|---|---|
| `name` | 厂商显示名 |
| `version` | Semver，格式 `AA.BB.CC.DD`；解析失败则整个厂商加载失败 |
| `force_update` / `description` / `url` | 更新相关元信息；`description` 只写日志 |
| `*_list[].name` | 清单项名称。**对 machine_model 有语义**：它就是 `printer_model` 要匹配的型号 ID |
| `*_list[].sub_path` | 相对 `<Vendor>/` 的路径 |

- 键名比较不区分大小写（`boost::iequals`）。
- **清单顺序有语义**：继承的父项必须排在子项前面（见 5.2）。
- 对 process / filament / machine，清单里的 `name` 只是提示，真正的预设名取文件内部的 `"name"` 字段（见 8.3）。

### 3.2 OrcaFilamentLibrary 特例

`OrcaFilamentLibrary.json` 的 `filament_list` 是空数组。加载器发现清单为空时，会扫描磁盘自动生成清单（共 274 个文件），顺序是：

1. `filament/base/` 下递归扫描到的文件；
2. `filament/` 顶层的文件；
3. `filament/` 下其余子目录，按目录名排序后逐个递归扫描。

每一组内部先按优先级排序，同级再按路径字典序：

| 优先级 | 规则 |
|---|---|
| 0 | 文件名为 `fdm_filament_common` |
| 1 | 文件名以 `@base` 结尾 |
| 2 | 文件名以 `_common` 结尾 |
| 3 | 其他 |

这种情况下清单项的 `name` 用的是**文件名**，不是文件内的 `"name"`。

源码：`PresetBundle.cpp` 中的 `append_json_subfiles` lambda，以及 `if (vendor_name == ORCA_FILAMENT_LIBRARY && filament_subfiles.empty())` 分支。

---

## 4. 机型文件 `machine_model`

位于 `<Vendor>/machine/`，由 `machine_model_list` 引用。它**不是预设**，只用来构造 `VendorProfile::PrinterModel`。

| 字段 | 出现次数 / 326 | 说明 |
|---|---|---|
| `type` | 326 | `"machine_model"` |
| `name` | 326 | 机型名 |
| `model_id` | 325 | 机型 ID |
| `nozzle_diameter` | 326 | **分号分隔**的变体列表，如 `"0.4;0.2;0.6;0.8"`；`printer_variant` 必须是其中之一 |
| `machine_tech` | 326 | 以 `SL` 开头算 SLA，否则算 FFF |
| `family` | 326 | 机型系列 |
| `bed_model` / `bed_texture` | 323 / 301 | 热床 STL / 贴图文件名 |
| `hotend_model` | 268 | 热端模型 |
| `default_materials` | 306 | 分号分隔的默认耗材名，加载时会去重、排序 |
| `url`、`default_bed_type`、`bed_exclude_area` 等 | 少量 | 加载器会忽略其中大部分字段 |

`nozzle_diameter` 为空或解析不出任何变体的机型会被丢弃。

---

### 4.1 机型资源查找与工作台检查

以下规则于 M13 对照当前检出的 C++ 源码确认；资源不是机型 JSON 的旁置文件，而是相对于厂商资源目录。

- `PresetUtils::system_printer_bed_model` / `system_printer_bed_texture`（`src/libslic3r/Preset.cpp:4195`）：先查 `<data_dir>/vendor/<Vendor>/<字段值>`，不存在时使用 `<resources_dir>/profiles/<Vendor>/<字段值>`。
- `PresetUtils::system_printer_hotend_model`（同文件 `4219`）：先走同样两级查找，仍不存在或字段为空时回退 `<resources_dir>/profiles/hotend.stl`。
- 设置向导封面（`src/slic3r/GUI/WebGuideDialog.cpp:1312`，`WebPresetDialog.cpp` 有相同规则）：以 `machine_model_list` 中的名称拼接 `<Model>_cover.png`，先查 `<resources_dir>/profiles/<Vendor>/`，再回退 `<resources_dir>/web/image/printer/`。Plater 侧栏缩略图另有基于预设名和占位图的显示逻辑，不据此判断机型是否可加载。

工作台只管理用户打开的 profiles 副本，不读取或改写 OrcaSlicer 的用户数据目录。因此 M13 对副本中缺失的资源给出预览提醒，说明可能的回退位置，不把它报告为新的全仓库加载错误，也不生成、复制、重命名或删除资源文件。资源字段需是厂商目录内的相对路径。

M13 表单接受普通分号列表，去除元素首尾空白再保存；暂不接收 C 风格引号/转义。此限制只属于编辑入口，不改变本节记录的 OrcaSlicer 加载语义。

---

## 5. 预设文件（process / filament / machine）

### 5.1 元信息字段

`ConfigBase::load_from_json` 会把下列键从配置参数里分离出来，单独存成元信息。其余所有键都按配置参数处理。

| 键 | 说明 |
|---|---|
| `name` | **预设的唯一标识**，同一类型内全局唯一（跨厂商也一样，见 8.2） |
| `type` | `process` / `filament` / `machine`，应当和所在清单一致 |
| `from` | 系统配置应为 `"system"`；用户配置为 `"User"`。现有数据不统一，见 8.1 |
| `instantiation` | `"true"`：用户可选；`"false"`：只用来被继承的模板。缺失或取其他值都会记为错误 |
| `inherits` | 父预设名；空字符串或缺失表示没有父项 |
| `setting_id` | 云同步和统计用的 ID；`blacklist.json` 里登记的是废弃 ID |
| `filament_id` | 仅耗材使用。缺失时依次尝试：沿父链继承 → 用 `setting_id` 兜底；都没有就报错 |
| `description` | 描述 |
| `renamed_from` | 旧名列表，分号分隔的 C 风格转义字符串 |
| `version` / `url` / `is_custom_defined` | 少见 |

此外有几类**以配置参数身份存储、但对工具有特殊语义**的键：

| 键 | 用于 |
|---|---|
| `compatible_printers` / `compatible_printers_condition` | 耗材、工艺与打印机的兼容性（见 6） |
| `compatible_prints` / `compatible_prints_condition` | 耗材与工艺的兼容性 |
| `printer_model` / `printer_variant` | machine 必填，必须指向本厂商的机型和变体 |
| `alias` | 显示别名；没写时由 `name` 推导（见 5.3 第 8 步）。注意：这个 fork 的 `print_config_def` 里没有 `alias`，JSON 里写了也会被当作未知键忽略，所以实际上总是推导出来的。现有数据没有用到它 |
| `base_id` / `user_id` / `updated_time` | 仅用户配置使用（写在 `.info` 文件中），系统配置里没有 |

### 5.2 参数值的编码

JSON 里**只会出现两种值形态**：字符串，或字符串数组。现有数据中没有数字、布尔、`null` 或嵌套对象。

| C++ 类型 | JSON 示例 | 备注 |
|---|---|---|
| bool | `"1"` / `"0"` | |
| int / float | `"8"`、`"0.82"` | 解析时强制使用 C locale |
| percent | `"15%"` | |
| floatOrPercent | `"0.4"` 或 `"110%"` | |
| enum | `"tree(auto)"` | 值必须在枚举表里。GUI 加载系统配置时用 `EnableSystemSilent` 规则，未知的枚举值和布尔值会被静默替换成默认值；只有 `EnableSilentDisableSystem` 规则下才会报错 |
| string | `"48x48/PNG, 300x300/PNG"` | G-code 等多行文本使用 `\n` 转义 |
| point 列表 | `["0x0", "270x0", "270x270", "0x270"]` | 如 `printable_area`、`bed_exclude_area` |
| vector（按挤出机或耗材） | `["0.4", "0.4"]` | 如 `nozzle_diameter` |
| nullable vector | `["nil"]` | 381 个文件用到了 `nil` |

**数组的规范化规则**（`Config.cpp` 中的 `load_from_json`）：

- 标量参数如果写成了数组：只有一个元素就取这个元素；多个元素且全部相同，就折叠成一个值并记一条警告。现有数据里大量标量写成了单元素数组。
- 非字符串类型的 vector：各元素用 `,` 拼接后反序列化。
- `coStrings` 类型的 vector：每个元素做 C 风格转义并加引号，再用 `;` 拼接。
- 数组里出现非字符串元素时，**整个键会被丢弃**并记录错误。

**旧键名和未知键**：键名会先经过 `handle_legacy` 转换。它会改名或丢弃过时的键，最后把 `print_config_def` 里没有的键都清空，所以**未知键会被静默忽略**，不会报错；`ConfigOptionDef::aliases` 实际上用不到。数组值的键会经过两次 `handle_legacy`：第一次值为空字符串，只用来确定键名；第二次在 `set_deserialize` 里用拼接后的值。

**值解析失败**：标量数值（int / float / percent 等）完全解析不了时，`set_deserialize` 抛出 `BadOptionValueException`，`load_from_json` 捕获后返回失败原因，**整个厂商加载失败**。数值后面多出的文字会被忽略（`"0.2mm"` 读成 0.2，`"35%"` 写在普通 float 上读成 35）。vector 的元素解析不了时不报错，直接读成 0。非 nullable 的 vector 里的 `nil` 元素会被跳过。

另外有个别特殊值会触发联动改写，例如 `support_type: "hybrid(auto)"` 会同时设置 `support_style`，`wall_infill_order` 的某些取值会设置 `is_infill_first`。

**值的类型信息不在 JSON 里**，定义在 `PrintConfig.cpp` 的 `ConfigOptionDef` 中。本项目用 OrcaSlicer 仓库里的 `src/dev-utils/Snapmaker_Orca_schema_export.cpp` 导出 `schema.json`（见 9.2 和 README）。

### 5.3 加载与继承算法（核心）

入口：`PresetBundle::load_system_presets_from_json` → `PresetBundle::load_vendor_configs_from_json`。下面用伪代码描述，按语义整理，不逐行对应源码：

```
vendors = 目录下所有 <Vendor>.json（目录遍历顺序，未定义）
把 OrcaFilamentLibrary 移到第一个
lib_maps = {}                                   # 公共耗材库中耗材的已解析配置

for vendor in vendors:
    manifest = 读取索引文件（OrcaFilamentLibrary 清单为空时扫描磁盘，见 3.2）
    models   = 解析 machine_model_list            # 见第 4 节

    for section in [process, filament, machine]:  # 固定顺序；每一类有自己的 local 表，类与类之间清空
        local = {}                                # name -> 已解析的完整配置
        for item in manifest[section]:            # 严格按清单顺序
            raw  = 读取 JSON，分离元信息和参数
            if raw.inherits 非空:
                parent = local[raw.inherits]
                      ?? lib_maps[raw.inherits]   # 唯一的跨厂商来源
                      ?? 报错 "Can not find inherits"
            else:
                parent = 该类型的默认配置           # machine 按 printer_technology 选择默认
            cfg = copy(parent); cfg.apply(raw.params)     # ①

            if instantiation == "false" and vendor != "Template":
                local[name] = cfg; continue               # ② 只当模板，不生成预设

            Preset::normalize(cfg)                        # ③
            remove_invalid_keys(cfg, parent)              # ④
            if section == machine: 校验 printer_model / printer_variant    # ⑤
            if 同类型中已有同名预设: 报错 "duplicated defines"              # ⑥
            if section == filament: 确定 filament_id，缺失则报错            # ⑦
            推导 alias / renamed_from                                       # ⑧
            生成 Preset(is_system=true, vendor, version=厂商版本)
            local[name] = cfg

        if vendor == OrcaFilamentLibrary and section == filament:
            lib_maps = local

    把本厂商合并进总 Bundle；同类型同名预设记为 duplicate 错误
```

要点：

1. **整键覆盖，不按元素合并**（①）。子配置里的 `["0.4","0.6"]` 会整个替换父配置的数组，不会逐元素合并。
2. **继承只能在同厂商、同类型内进行**；唯一例外是任意厂商的配置都可以继承 OrcaFilamentLibrary 的耗材。目前有 65 个厂商耗材这样做。
   - 实现上，查 `lib_maps` 时并不检查当前是哪一类，但它里面只有耗材，所以实际上只有耗材能命中。
3. **父项必须在清单里排在前面**。加载是一次顺序扫描，`local` 表里只有已经处理过的项。目前数据里没有违反这一条的情况。
4. **模板也保存的是已解析的完整配置**（②），所以多级继承会自然叠加。
5. **Template 厂商例外**：`instantiation: "false"` 的模板也会生成预设，耗材也不强制要求 `filament_id`。
6. `normalize`（③）会根据挤出机或耗材数量，统一调整各个按挤出机存值的 vector 的长度。
7. `remove_invalid_keys`（④）会删除不属于该类型的键并报错，判断依据是「父配置或该类型默认配置里有没有这个键」。新项目判断「哪些键属于哪一类」，应以 `Preset::print_options()`、`filament_options()`、`printer_options()` 为准。
8. **别名推导**（⑧）：如果没有写 `alias`，就取 `name` 里第一个 `@` 之前的部分并去掉尾部空格；同时自动在 `renamed_from` 里补一项「去掉 `@` 后的原名」。例如 `Generic ABS @U1 0.2 nozzle` 的别名是 `Generic ABS`。
9. **错误处理分两级**：
   - 单个文件出错时，`parse_subfile` 返回失败原因，调用方会**抛出 `ConfigurationError`，导致整个厂商加载失败**；
   - 只有在正常（非校验）模式下，外层才会捕获这个异常、累积错误信息并继续加载下一个厂商。
   - 所以校验器应把「找不到父项」「printer_model 无效」「重复定义」「缺少 filament_id」都标为**致命错误**。

---

## 6. 兼容性匹配

源码：`Preset.cpp` 中的 `is_compatible_with_printer`、`is_compatible_with_print`。

### 6.1 耗材或工艺与打印机

按以下顺序判断，命中即返回：

1. 如果是 OrcaFilamentLibrary 的耗材，且当前打印机在它的 `m_excluded_from` 里（见 6.2）→ **不兼容**。
2. `compatible_printers` 为空且 `compatible_printers_condition` 非空 → 计算表达式，结果即为答案。
   - 计算时可用的变量：打印机的全部配置，外加 `printer_preset`（打印机预设名）和 `num_extruders`；
   - 表达式解析出错时，返回「兼容」。
3. 满足以下任一条件 → **兼容**：
   - 是默认预设；
   - 当前打印机名为空；
   - `compatible_printers` 为空；
   - 打印机名在 `compatible_printers` 里；
   - 当前打印机是用户预设，且它的 `inherits`（只看直接父项）在 `compatible_printers` 里。

另外两点：

- **允许跨厂商兼容**：Orca 把原先的厂商检查注释掉了。
- `PresetBundle::has_errors()` 要求：除 OrcaFilamentLibrary 外，所有系统耗材的 `compatible_printers` 都不能为空。

### 6.2 OrcaFilamentLibrary 的排除机制

源码：`PresetCollection::update_library_profile_excluded_from`。

规则：如果公共耗材库里某个耗材的 `compatible_printers` 为空，而某个具体厂商提供了**同别名**的耗材并列出了 `compatible_printers`，那么这些打印机会被加入公共耗材的 `m_excluded_from`。

效果：在这些打印机上，用户只会看到厂商自己的版本，不会同时看到公共库的通用版本。

### 6.3 工艺与耗材

`is_compatible_with_print` 的逻辑和 6.1 相同，只是换成 `compatible_prints` / `compatible_prints_condition`，表达式针对工艺配置计算。

### 6.4 条件表达式

目前有 95 个工艺预设使用了 `compatible_printers_condition`。语法来自 `PlaceholderParser`，示例：

```
printer_notes=~/.*PRINTER_MODEL_COREONE.*/ and nozzle_diameter[0]==0.6
```

包含正则匹配 `=~`、`and` / `or` / `not`、数组下标和数值比较。新项目第一版可以先只支持这个子集；遇到无法求值的表达式，就把结果显示为「需人工确认」。

---

## 7. 不是预设的 JSON 文件

| 文件 | 作用 | 新项目怎么处理 |
|---|---|---|
| `blacklist.json` | 记录废弃的 `setting_id`（`filament` / `process` 两个数组），供 `check_unused_setting_id.py` 使用 | 跳过；可用于校验 setting_id |
| `BBL/cli_config.json` | 命令行模式用的降级检查和机器限制（`src/Snapmaker_Orca.cpp` 读取） | 跳过 |
| `Snapmaker/filament/filament_hot_bed_nozzles.json` | 各热床类型支持或警告的耗材类别，由 `FilamentHotBedNozzleRules.cpp` 读取 | 跳过；也可以作为扩展功能展示 |
| `Snapmaker/filament/filament_compatibility.json` | 耗材类别之间的混打兼容表 | 同上 |
| `Snapmaker/filament/filament_allow_list.json`、`filaments_colours.json` | Snapmaker 的辅助数据；在 `src` 里没有找到直接引用这两个文件名的代码，加载位置未确认 | 跳过 |

工具识别预设文件的规则应当是「在清单中出现」，而不是「后缀是 `.json`」。

---

## 8. 当前数据中的问题（可以直接当作校验器的测试用例）

### 8.1 `from` 字段取值不统一

| 值 | 数量 |
|---|---|
| `"system"` | 6998 |
| 缺失 | 73 |
| `"User"` | 21 |
| `""` | 2 |
| `"System"` | 1 |

加载器不读这个字段（系统配置一律设为 `is_system = true`），所以它不影响运行，但属于数据质量问题。

### 8.2 跨厂商重复的预设名（10 个，都在 BBL 和 Snapmaker 之间）

`Generic PA`、`Generic PA-CF`、`Generic TPU`、`Generic ABS`、`Generic PC`、`Generic PETG`、`Generic PLA`、`Generic PLA Silk`、`Generic PLA-CF`、`Generic PVA`。

合并厂商时，它们会被报告为 `Found duplicated settings in vendor ...`，并计入错误数。最终生效的是哪一份，取决于目录遍历顺序。**这一项和 Snapmaker 直接相关，值得优先确认。**

### 8.3 清单名与文件内 `name` 不一致（6 个，都在 OrcaFilamentLibrary）

例如文件名 `Bambu Support For PA PET @base.json`，文件内的名字是 `Bambu Support For PA/PET @base`。原因是文件名里不能有 `/`。这不算错误，但说明**所有查找都必须使用文件内的 `name`**。

### 8.4 不在任何清单里的 JSON（457 个，不会被加载）

| 厂商 | 数量 | 说明 |
|---|---|---|
| BBL | 448 | 大量 `filament/<Brand>/…` 文件没有列入清单 |
| Snapmaker | 5 | 其中 4 个是第 7 节的规则文件（正常）；**`process/0.24 Standard @Snapmaker (0.8 nozzle).json` 没有列入清单，因此不会被加载**，需要确认是不是漏加了 |
| InfiMech | 2 | |
| Phrozen | 1 | |
| Prusa | 1 | |

### 8.5 文件开头有 UTF-8 BOM（2 个，都在 Snapmaker）

- `Snapmaker/process/fdm_process_common.json`
- `Snapmaker/process/0.20mm Standard @Snapmaker U1 (0.4 nozzle).json`

nlohmann::json 会自动跳过 BOM，所以 Orca 能正常加载；但 **Rust 的 `serde_json` 和 Python 的 `json` 遇到 BOM 都会解析失败**。新项目读取文件时要先去掉 BOM，校验器可以把它报为警告。

### 8.6 `compatible_printers` 引用了不存在的打印机（13459 个引用中有 581 个）

| 厂商 | 数量 |
|---|---|
| Qidi | 404 |
| Blocks | 67 |
| Elegoo | 63 |
| OrcaArena | 22 |
| Afinia | 12 |
| 其他 | 13 |

例如 `Afinia PLA` 引用了 `Afinia H400 Pro 0.4 nozzle`，但仓库里没有这台打印机。

判断依据：被引用的名字不在任何厂商的可选 machine 预设名里。这种引用不会导致加载失败，只是永远不会匹配上。

### 8.7 没有发现问题的项（可以作为回归基线）

- 找不到父项：0
- 父项排在子项之后：0
- `type` 与所在清单不一致：0
- `printer_model` / `printer_variant` 为空或无效：0
  - 判断用的是沿继承链解析后的值。134 个可选 machine 文件自己没写这两个字段，是从父项继承来的。
- 清单列出但文件不存在：0

---

### 8.8 工作台的多余覆盖值提示（M14）

这是一项工作台新增的 info 提示，不是 OrcaSlicer 的加载错误。子预设自己写出的值与其父项的最终值按原始字符串/数组完全相同时，报告 `redundant-override`；字符串与单元素数组、不同数字拼写、数组顺序均不等同。根预设和不完整的继承链不参与比较；有 schema 时，不属于该预设类型的键（OrcaSlicer 会丢弃它们，已另有提示）也不参与比较。有 schema 时，父项未显式设置的键可以来自默认层。

在同一个锁定数据版本上，不带 schema 为 **56,926** 项，带 bundled schema 为 **73,253** 项。独立参考统计：

```bash
python3 -I docs/reference/profile_format_stats.py ../OrcaSlicer/resources/profiles
python3 -I docs/reference/profile_format_stats.py ../OrcaSlicer/resources/profiles crates/profile-core/schema/schema.json
```

该提示不修改文件。删除相同覆盖后，子项会跟随后续的父项变化；是否删除或把兄弟项共同值提到父项由用户决定。

---

## 9. 对新项目的设计建议

### 9.1 数据模型草案（Rust）

```rust
enum PresetKind { Process, Filament, Machine }

struct VendorIndex {
    name: String,
    version: String,
    machine_models: Vec<ManifestItem>,
    process: Vec<ManifestItem>,
    filament: Vec<ManifestItem>,
    machine: Vec<ManifestItem>,
}
struct ManifestItem { list_name: String, sub_path: PathBuf }

struct RawPreset {                      // 文件原样内容，保留键的顺序
    path: PathBuf,
    vendor: String,
    kind: PresetKind,
    meta: PresetMeta,                   // name / inherits / instantiation / setting_id / ...
    params: IndexMap<String, RawValue>,
    had_bom: bool,
}
enum RawValue { Str(String), List(Vec<String>) }

struct ResolvedPreset {
    name: String,
    chain: Vec<PresetId>,               // [自己, 父, 祖父, ...]，最后一项可能来自 OrcaFilamentLibrary 或类型默认配置
    values: IndexMap<String, (RawValue, Origin)>,   // Origin：这个值来自继承链上的哪一层
    alias: String,
}
```

### 9.2 实现建议

1. **严格复刻第 5.3 节的顺序语义**：一个厂商一个上下文，每一类一张 `local` 表，按清单顺序处理，`lib_maps` 是唯一的跨厂商来源。不要按名字全局构建继承图，否则会掩盖「父项排在后面」这一类错误。
2. **先把值当字符串处理**。类型信息来自一份导出的 `schema.json`：写一个小的 C++ 程序链接 libslic3r，遍历 `print_config_def`，输出每个键的 `type`、`default`、`min`/`max`、枚举值、`label`、`tooltip`、`category`、`mode`、`nullable`，以及 `Preset::print_options()`、`filament_options()`、`printer_options()` 这三个键集合。
3. **「没有父项」时的基准值**：类型默认配置无法从 JSON 推出，需要在 schema 里一并导出每个键的默认值。
4. **保存时尽量减小 diff**：保留原有的键顺序、缩进和是否单元素数组的写法，这样提交到 Git 时 diff 最小。可以用 `serde_json` 的 `preserve_order` 特性，或者直接用 `IndexMap`。
5. 读取时**容忍 BOM**，键名比较**不区分大小写**（和 `boost::iequals` 的行为一致）。
6. 第一版兼容性计算只实现名单匹配和 6.2 的排除机制；条件表达式第二阶段再做（见 6.4）。
7. **回归测试**：把第 1 节和第 8.7 节的统计数字写进测试断言，数据有变化时一眼就能看出来。

---

## 10. 源码索引

| 内容 | 位置 |
|---|---|
| 厂商遍历、OrcaFilamentLibrary 置顶、合并 | `src/libslic3r/PresetBundle.cpp`：`load_system_presets_from_json`（约第 1440–1530 行） |
| 单个厂商加载、清单解析、机型解析 | `src/libslic3r/PresetBundle.cpp:3284` `load_vendor_configs_from_json` |
| 单个预设解析和继承（`parse_subfile`） | 同一函数内，约第 3600–3860 行 |
| 公共库 `lib_maps`（`m_config_maps`） | `PresetBundle.cpp` 约第 3905 行；`PresetBundle.hpp:199` |
| 厂商合并与重复检测 | `src/libslic3r/PresetBundle.cpp:1657` `merge_presets` |
| JSON 值解析和数组规范化 | `src/libslic3r/Config.cpp:832` `ConfigBase::load_from_json` |
| 元信息键名常量 | `src/libslic3r/Preset.hpp:39-70` |
| `normalize` / `remove_invalid_keys` | `src/libslic3r/Preset.cpp:385` / `:504` |
| 兼容性判断 | `src/libslic3r/Preset.cpp:743` / `:781` / `:814` |
| 公共库排除机制 | `src/libslic3r/Preset.cpp:3466` |
| 每类的键集合 | `src/libslic3r/Preset.cpp:1185-1195` |
| `has_errors` | `src/libslic3r/PresetBundle.cpp:4513` |
| 现有 C++ 校验工具 | `src/dev-utils/Snapmaker_Orca_profile_validator.cpp` |
| 现有 Python 检查脚本 | `scripts/orca_extra_profile_check.py`（检查 compatible_printers、default_materials、命名一致性、filament_id、废弃键） |
