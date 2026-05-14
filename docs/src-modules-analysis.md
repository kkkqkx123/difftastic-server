# src 目录模块分析

## 项目概述

Difftastic 是一个基于语法树（syntactic diff）的差异比较工具。它使用 tree-sitter 解析器将源代码解析为 AST，然后在语法树层面进行结构化比较，生成更精确、更可读的 diff 结果。

---

## 顶层模块

### `main.rs` — 程序入口与核心编排
项目入口点。解析命令行参数后分发到四种模式：
- **`Mode::Diff`** — 标准的文件/目录差异比较（核心路径）
- **`Mode::DiffFromConflicts`** — 从 git 冲突标记合并后的文件还原两侧内容并比较
- **`Mode::DumpTreeSyntax / DumpSyntax / DumpSyntaxDot`** — 调试模式，输出 tree-sitter AST
- **`Mode::ListLanguages`** — 列出所有支持的语言
- **`Mode::GitHasUnmergedFile`** — 输出未合并路径信息

核心函数 `diff_file_content()` 是整个 diff 流程的编排者：语言猜测 → 语法解析 → 标记未变更节点 → Dijkstra 最短路径算法 → slider 修正 → 生成 hunk。

### `options.rs` — CLI 参数解析与配置
使用 `clap` 解析命令行参数，定义关键配置结构体：
- **`DisplayOptions`** — 显示相关的配置（色彩模式、显示模式、终端宽度、上下文行数、语法高亮等）
- **`DiffOptions`** — 算法相关的配置（图限制、字节限制、解析错误限制、检查模式、忽略注释等）
- **`Mode`** 枚举 — 涵盖所有 CLI 子命令

### `files.rs` — 文件读取与类型探测
- 处理 `NamedPath` / `Stdin` / `DevNull` 三种文件参数
- `guess_content()` — 根据魔术字节和文件扩展名判断文件是文本还是二进制
- 目录比较时使用 `ignore::Walk` 遍历文件，通过 rayon 并行比较

### `lines.rs` — 行操作工具
- `split_on_newlines()` — 按 `\n` 或 `\r\n` 分割字符串
- `MaxLine` trait — 计算字符串的最大行号（0-indexed）
- `byte_len()` / `format_line_num()` / `is_all_whitespace()` 等辅助函数

### `words.rs` — 单词拆分工具
- `split_words()` — 将字符串拆分为单词和非单词字符（`"foo..bar23" → ["foo", ".", ".", "bar23"]`）
- `split_words_and_numbers()` — 同上但区分数字和字母（`"bar23" → ["bar", "23"]`）

### `hash.rs` — 高性能哈希类型别名
- `DftHashMap` — 使用 FxHasher 的 `hashbrown::HashMap`，用于热路径上的快速哈希
- `DftHashSet` — FxHashSet 的别名

### `constants.rs` — 基础枚举
定义 `Side::Left / Right` 枚举，表示 diff 的左侧（旧文件）和右侧（新文件）。

### `exit_codes.rs` — 退出码常量
- `EXIT_SUCCESS = 0` — 未发现变更
- `EXIT_FOUND_CHANGES = 1` — 发现变更
- `EXIT_BAD_ARGUMENTS = 2` — 参数错误

### `version.rs` — 版本信息
构建时从 `CARGO_PKG_VERSION` 等环境变量读取版本、commit hash、rustc 版本信息。

### `summary.rs` — Diff 结果数据结构
定义核心数据结构：
- **`DiffResult`** — 包含显示路径、额外信息、文件格式、源码内容、hunk 列表、变更位置、字节/syntax 变更标记
- **`FileContent`** — `Text(String)` 或 `Binary`
- **`FileFormat`** — `SupportedLanguage(Language)`、`PlainText`、`TextFallback`、`Binary`

### `gitattributes.rs` — Git diff/binary 属性查询
调用 `git check-attr diff binary` 获取文件的 gitattributes 配置，返回值可为 `AssumeText`、`AssumeBinary` 或 `Unspecified`。支持缓存以避免重复执行 git 命令。

### `conflicts.rs` — Git 冲突标记解析
解析 `<<<<<<<` / `|||||||` / `=======` / `>>>>>>>` 等冲突标记，还原出 LHS（当前分支）和 RHS（合并分支）的原始内容。支持 `diff3` 冲突风格。

### `line_parser.rs` — 纯文本回退差异比较
当 difftastic 不支持某语言或超过限制时，退化为基于行的文本差异比较：
1. 按行分割 → 逐行拆分为单词 → 使用 `lcs_diff::slice_by_hash()` 进行单词级 LCS 差异
2. 合并连续 Novel 区间 → 生成 `MatchedPos` 位置信息

---

## `diff/` 模块 — 语法树差异计算核心

### `diff/mod.rs` — 模块声明
导出 `changes`、`dijkstra`、`lcs_diff`、`sliders`、`unchanged` 子模块。

### `diff/changes.rs` — 变更标记类型
定义 `ChangeKind` 枚举，标记语法节点是：
- `Unchanged` — 完全未变更（含反向指针）
- `Novel` — 新增/删除
- `IgnoredPunctuation` — 可忽略的标点（如尾逗号）
- `ReplacedComment / ReplacedString` — 注释/字符串被替换
- `ChangeMap` — 从 `SyntaxId` 到 `ChangeKind` 的映射表

### `diff/dijkstra.rs` — Dijkstra 最短路径算法
核心 diff 算法。将两个 AST 之间的差异比较建模为有向无环图的最短路径问题：
1. 每个顶点表示 LHS 和 RHS 中待匹配的节点指针
2. 边表示可能的操作（匹配、删除 LHS 节点、插入 RHS 节点等）
3. 使用 radix heap（基数堆）实现 Dijkstra 算法寻找最小代价路径
4. 通过 `DFT_GRAPH_LIMIT` 环境变量限制搜索规模
5. 代价函数和 `normalized_levenshtein` 相似度计算在 `graph.rs` 中详细实现

### `diff/graph.rs` — 图结构与邻接关系
定义 `Vertex` 和 `Edge`：
- **`Vertex`** — 包含 LHS/RHS 语法指针、括号栈状态、前驱指针
- **`Edge`** — 七种边类型：`Match`、`NovelAtomLHS/RHS`、`EnterDelimiterLHS/RHS`、`LeaveDelimiterDelimited/Novel`
- `set_neighbours()` — 根据当前顶点状态生成可达邻居
- `populate_change_map()` — 将最短路径回溯结果写入 ChangeMap
- 代价计算使用编辑距离（Levenshtein）和原子相似度

### `diff/lcs_diff.rs` — LCS（最长公共子序列）线性 diff
使用 `wu-diff` crate 实现 Wu 算法：
- `slice()` — 直接比较
- `slice_by_hash()` — 通过哈希间接比较（适用于大字符串，避免昂贵的相等比较）
- 用于纯文本回退和 `unchanged` 模块中的子节点序列匹配

### `diff/unchanged.rs` — 预标记未变更节点
在运行完整 Dijkstra 之前，通过快速扫描找出明显未变更的节点：
1. `shrink_unchanged_at_ends()` — 从序列两端收缩已匹配部分
2. `split_mostly_unchanged_toplevel()` — 用 LCS 切分"大部分未变"的段
3. `split_unchanged()` — 递归标记结构完全相同的子树为 Unchanged
4. 减少输入规模，提升后续 Dijkstra 算法的性能

### `diff/sliders.rs` — Slider 修正
修正 diff 结果中 Novel 标记位置不理想的问题。例如当新增代码与已有代码在语义上等价时，通过滑动 Novel 标记到更合适的行/位置来提升可读性：
1. `fix_all_sliders_one_step()` — 单步滑动
2. `fix_all_nested_sliders()` — 嵌套结构滑动
3. `drop_ignored_punctuation()` — 丢弃可忽略的尾部标点

### `diff/stack.rs` — 持久化栈
使用 bumpalo arena 分配器实现的不可变/持久化栈（persistent stack），用于图遍历时跟踪括号嵌套状态。相比于 `rpds::Stack` 更快且内存更高效。

---

## `display/` 模块 — 差异结果展示

### `display/mod.rs` — 模块声明
导出 `context`、`hunks`、`inline`、`json`、`side_by_side`、`style` 子模块。

### `display/context.rs` — 上下文行计算
计算差异附近哪些行也应该展示给用户：
- `all_matched_lines_filled()` — 构建完整的 LHS↔RHS 行映射
- `add_ends()` — 补充文件首尾的空行
- `add_context()` — 在变更行前后扩展指定数量的上下文行
- `compact_gaps()` — 合并间隔较小的上下文区域
- `ensure_contiguous()` — 保证行映射连续
- `match_preceding_blanks()` — 匹配变更前的空白行

### `display/hunks.rs` — Hunk （差异块）计算
将位置信息转换为展示用的 Hunk：
- `matched_pos_to_hunks()` — 将 `MatchedPos` 列表转换为 `(Option<LineNumber>, Option<LineNumber>)` 行对
- `merge_adjacent()` — 合并距离较近的 Hunk（`MAX_DISTANCE = 4` 行内）
- 处理行号重复、不连续等边界情况

### `display/style.rs` — 颜色与样式
负责 ANSI 终端颜色应用：
- `apply_colors()` — 对源码应用语法高亮和 diff 标记色
- `novel_style()` — 根据背景色选择新增/删除行的颜色
- `replace_tabs()` — 处理制表符显示宽度
- `width_respecting_tabs()` — 计算考虑制表符的显示宽度
- 头信息格式化（文件名、文件格式等）

### `display/inline.rs` — 内联（unified）模式
类似 `git diff` 的统一格式输出。左右两侧的变更在同一个视图中通过 `+`/`-` 前缀标记展示。

### `display/side_by_side.rs` — 并排（双栏）模式
两栏对比展示：
- 左侧显示旧文件，右侧显示新文件
- 变更行高亮，未变行对齐
- `lines_with_novel()` — 找出包含 Novel 内容的行
- 支持 `--display side-by-side-show-both` 模式

### `display/json.rs` — JSON 输出模式
将 diff 结果序列化为 JSON，供其他工具消费：
- 每条变更包含行号、状态（unchanged/changed/created/deleted）
- 支持单文件和目录两种输出格式

---

## `parse/` 模块 — 语言解析

### `parse/mod.rs` — 模块声明
导出 `guess_language`、`syntax`、`tree_sitter_parser` 子模块。

### `parse/guess_language.rs` — 语言识别
基于文件名后缀和 shebang 行（如 `#!/bin/bash`）判断文件语言：
- `Language` 枚举 — 涵盖 100+ 种支持的语言（Ada、Bash、C、C++、Go、Java、Python、Rust 等）
- `guess()` — 根据路径和内容猜测语言
- `language_globs()` — 获取语言对应的 glob 匹配模式
- 支持 `--language-override` 用户自定义覆盖

### `parse/syntax.rs` — 语法树数据结构
定义 difftastic 自身的语法树表示：
- **`Syntax` 枚举** — `List { open, children, close }` 和 `Atom { value, kind }`
- **`SyntaxInfo`** — 节点元数据（前后兄弟节点、父节点、层级深度、唯一 ID、内容 ID 等）
- **`MatchedPos`** — 匹配后的位置信息，记录 LHS 到 RHS 或 RHS 到 LHS 的行号映射
- `init_all_info()` — 初始化所有节点的深度优先顺序、唯一 ID
- `init_next_prev()` — 初始化兄弟关系和前后关系
- `change_positions()` — 根据 ChangeMap 生成行级别位置标记

### `parse/tree_sitter_parser.rs` — Tree-sitter 解析器集成
连接 tree-sitter 生态系统的桥梁：
- 加载各语言的 tree-sitter 解析库
- `parse()` — 将源代码解析为 difftastic 的 `Syntax` 树
- `to_tree()` — 获取 tree-sitter 原始语法树
- 处理子语言内嵌（如 HTML 中的 JavaScript）
- `atom_nodes` / `delimiter_tokens` — 标记哪些 tree-sitter 节点应视为 Atom 或 List 结构
- 支持 `DFT_BYTE_LIMIT` 字节限制和 `DFT_PARSE_ERROR_LIMIT` 解析错误限制
- `comment_positions()` — 提取注释节点位置（用于 `--ignore-comments`）
- `print_tree()` — 调试用：输出 tree-sitter 原始树
