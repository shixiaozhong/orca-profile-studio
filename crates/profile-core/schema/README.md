# schema.json 的来源

`schema.json` 不是本项目编写的内容，而是从 OrcaSlicer 的源码导出的数据：

- **内容**：每个配置项的定义（类型、默认值、范围、枚举值、`label`、`tooltip`、类别和模式）、三类默认预设，以及 `Preset::*_options()` 键列表。它们来自 OrcaSlicer 的 `print_config_def` 和 `PresetBundle`。
- **来源版本**：文件里的 `source_commit`（当前为 `953509c23f`），即 [shixiaozhong/OrcaSlicer](https://github.com/shixiaozhong/OrcaSlicer)（[Snapmaker/OrcaSlicer](https://github.com/Snapmaker/OrcaSlicer) 的 fork）`personal/notes` 分支上的提交。
- **导出方式**：用 OrcaSlicer 仓库里的导出程序 `src/dev-utils/Snapmaker_Orca_schema_export.cpp` 生成，命令见仓库根目录的 `AGENTS.md` / `CLAUDE.md`。
- **许可**：OrcaSlicer 以 [GNU AGPL v3](https://github.com/SoftFever/OrcaSlicer/blob/main/LICENSE.txt) 授权，这份文件中的配置项名称、说明等内容适用 OrcaSlicer 的许可。本项目其余代码尚未选定 license。

更新 OrcaSlicer 后，重新运行导出程序覆盖这个文件，并同步 `source_commit`。
