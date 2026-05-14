# gRPC/HTTP 服务功能设计方案

## 1. 当前问题分析

### 1.1 每次 CLI 调用都重建一切的浪费

当前纯 CLI 架构下，每次 `diff_file_content()` 调用都从头构建整个 pipeline：

```
diff_file_content()
  └─ guess(path, src, overrides)          ← 文件路径 + 内容启发式判断语言
  └─ from_language(lang)                  ← 构建 TreeSitterConfig
  │    ├─ tree_sitter::Language::new(...)    ← 获取语言指针（轻量）
  │    ├─ ts::Query::new(...)               ← 【重】编译语法高亮 S-expression 查询
  │    └─ 填充 atom_nodes / delimiter_tokens / sub_languages 等
  └─ Arena::new()                         ← 新的 arena 分配器
  └─ to_tree_with_limit()
  │    └─ to_tree() x2
  │         └─ ts::Parser::new()          ← 【重】每次创建新 parser
  │         └─ parser.set_language()
  │         └─ parser.parse()             ← tree-sitter 解析
  └─ to_syntax_with_limit()
       └─ tree_highlights()               ← 对语法树应用高亮 query
       └─ all_syntaxes_from_cursor()      ← 转换为 difftastic Syntax 树
```

**每次调用的冗余开销：**

| 操作 | 浪费程度 | 原因 |
|------|---------|------|
| `ts::Query::new()` 编译高亮 query | **高** | 每次 `from_language()` 都重新编译，但同一语言的 query 字符串完全相同 |
| `ts::Parser::new()` | 中 | tree-sitter parser 内部会分配内存、初始化状态，完全可复用 |
| `to_tree()` 解析 | 必要 | 源码不同时必须解析，但 **parser 可复用** |
| `guess()` 语言检测 | 部分浪费 | 同一路径模式反复检测，但内容可能变化 |
| `Arena::new()` | 低 | bump allocator 创建成本很低 |

### 1.2 场景分析

**场景一：编辑器集成（如 Emacs `vc-diff`、VS Code 扩展）**
- 用户保存文件 → 编辑器调用 `difft old new` → 进程启动 → 全量初始化 → diff → 进程退出
- 每次保存都要重新加载所有 tree-sitter parser 库、编译 query
- 频繁调用时进程启动开销 + parser 初始化开销累积明显

**场景二：git 外部 diff 驱动**
- `git difftool` 对每个文件分别调用一次 `difft`
- 一个 PR 可能涉及 50+ 文件 → 50+ 次完全重复的初始化

---

## 2. 提议架构

### 2.1 目录结构

```
src/
├── main.rs                    # 条件编译入口，根据 feature 选择运行模式
├── api/                       # 新增：API 层
│   ├── mod.rs                 # 模块根，条件导出
│   ├── common.rs              # 公共类型：DiffRequest, DiffResponse, DiffService
│   ├── cli.rs                 # 从 main.rs 抽离的 CLI 逻辑
│   ├── grpc.rs                # gRPC 服务端（feature = "grpc"）
│   └── http.rs                # HTTP 服务端（feature = "http"）
├── diff/                      # 不变
├── display/                   # 不变
├── parse/                     # 不变（但 TreeSitterConfig 变为可缓存）
├── options.rs                 # 不变
├── files.rs                   # 不变
└── ...                        # 其他模块不变
```

### 2.2 Cargo.toml Feature 配置

```toml
[features]
default = ["cli"]
cli = []
http = ["dep:axum", "dep:tokio", "dep:tower", "dep:hyper"]
grpc = ["dep:tonic", "dep:tokio", "dep:tonic-build"]
server = ["http", "grpc"]

[dependencies]
# 仅 grpc feature 时引入
tonic = { version = "0.12", optional = true }
tonic-reflection = { version = "0.12", optional = true }

# 仅 http feature 时引入
axum = { version = "0.8", optional = true, features = ["json"] }
tower = { version = "0.5", optional = true }

# 共同依赖
tokio = { version = "1", optional = true, features = ["full"] }
serde = { version = "1.0", features = ["derive"] }           # 已有
serde_json = "1.0"                                             # 已有

[build-dependencies]
tonic-build = { version = "0.12", optional = true }
```

### 2.3 条件编译入口（main.rs）

```rust
// main.rs

// 根据 feature 选择运行时模式
#[cfg(feature = "grpc")]
fn run() {
    api::grpc::serve();
}

#[cfg(feature = "http")]
fn run() {
    api::http::serve();
}

#[cfg(not(any(feature = "grpc", feature = "http")))]
fn run() {
    api::cli::run();
}

fn main() {
    pretty_env_logger::try_init_timed_custom_env("DFT_LOG")
        .expect("...");

    #[cfg(unix)]
    reset_sigpipe();

    run();
}
```

---

## 3. 核心重构：DiffService

将 diff 的核心逻辑从 `diff_file_content` 函数中提取为有状态的服务对象，管理缓存的 `TreeSitterConfig`。

### 3.1 公共类型定义（`api/common.rs`）

```rust
use crate::parse::guess_language::{Language, LanguageOverride};
use crate::options::{DiffOptions, DisplayOptions};
use crate::summary::DiffResult;

/// API 层的 diff 请求。
/// 与 CLI 不同，内容直接通过字段传递，无需文件路径。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffRequest {
    pub lhs_content: String,
    pub rhs_content: String,
    /// 用于语言检测和显示的文件路径
    pub display_path: Option<String>,
    /// 可选的语言覆盖
    pub language_override: Option<String>,
    pub diff_options: Option<DiffOptions>,
    pub display_options: Option<DisplayOptions>,

    /// gRPC 专属：是否流式返回结果
    #[serde(skip)]
    pub stream: bool,
}

/// gRPC proto 对应的响应
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffResponse {
    pub display_path: String,
    pub file_format: String,
    pub has_syntactic_changes: bool,
    pub has_byte_changes: bool,
    pub lhs_byte_len: Option<usize>,
    pub rhs_byte_len: Option<usize>,

    /// Hunk 级别的变更数据
    pub hunks: Vec<HunkData>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HunkData {
    pub lhs_start_line: u32,
    pub rhs_start_line: u32,
    pub lhs_line_count: u32,
    pub rhs_line_count: u32,
    pub lines: Vec<LineData>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LineData {
    pub lhs_line: Option<String>,
    pub rhs_line: Option<String>,
    pub lhs_line_num: Option<u32>,
    pub rhs_line_num: Option<u32>,
    pub change_type: ChangeType,
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum ChangeType {
    Equal,
    Inserted,
    Deleted,
    Modified,
}
```

### 3.2 DiffService（`api/common.rs`）

```rust
use std::sync::Arc;
use crate::parse::tree_sitter_parser::{TreeSitterConfig, self as tsp};
use crate::parse::guess_language::{self, Language};
use crate::parse::syntax;
use crate::hash::DftHashMap;
use typed_arena::Arena;

/// 持有缓存的服务对象，用于 gRPC 和 HTTP 共享。
pub struct DiffService {
    /// TreeSitterConfig 缓存：Language 枚举值 → Arc<TreeSitterConfig>
    /// 避免重复编译 ts::Query
    config_cache: once_cell::sync::OnceCell<Mutex<DftHashMap<Language, Arc<TreeSitterConfig>>>>,
}

impl DiffService {
    pub fn new() -> Self {
        Self {
            config_cache: once_cell::sync::OnceCell::new(),
        }
    }

    fn get_config(&self, language: Language) -> Arc<TreeSitterConfig> {
        let map = self.config_cache.get_or_init(|| {
            Mutex::new(DftHashMap::default())
        });
        let mut map = map.lock().unwrap();
        map.entry(language)
            .or_insert_with(|| Arc::new(tsp::from_language(language)))
            .clone()
    }

    /// 执行一次 diff，返回结构化的 DiffResult。
    /// 与当前的 diff_file_content() 类似，但不涉及文件 I/O。
    pub fn diff(
        &self,
        lhs_content: &str,
        rhs_content: &str,
        display_path: &str,
        language_override: Option<Language>,
        overrides: &[(LanguageOverride, Vec<glob::Pattern>)],
        diff_options: &DiffOptions,
        display_options: &DisplayOptions,
    ) -> DiffResult {
        // 1. 语言侦探（如果未指定 override）
        let language = language_override
            .or_else(|| guess_language::guess(Path::new(display_path), rhs_content, overrides));

        // 2. 如果内容相同，提前返回
        if lhs_content == rhs_content {
            return /* ... early return ... */;
        }

        // 3. 使用或创建 TreeSitterConfig（核心缓存点）
        let config = language.map(|lang| (lang, self.get_config(lang)));

        // 4. 以下流程与现有 diff_file_content() 基本相同，
        //    但使用 cached config + 新 Arena
        match config {
            None => {
                // 纯文本回退
                let lhs_positions = line_parser::change_positions(lhs_content, rhs_content);
                let rhs_positions = line_parser::change_positions(rhs_content, lhs_content);
                /* ... */
            }
            Some((language, config)) => {
                let arena = Arena::new();
                match tsp::to_tree_with_limit(diff_options, &config, lhs_content, rhs_content) {
                    Ok((lhs_tree, rhs_tree)) => {
                        match tsp::to_syntax_with_limit(
                            lhs_content, rhs_content,
                            &lhs_tree, &rhs_tree,
                            &arena, &config, diff_options,
                        ) {
                            Ok((lhs, rhs)) => {
                                let mut change_map = ChangeMap::default();
                                /* unchanged -> dijkstra -> sliders -> change_positions */
                            }
                            Err(_) => { /* fallback */ }
                        }
                    }
                    Err(_) => { /* fallback */ }
                }
            }
        }

        // 5. 构建 DiffResult
        // ...
    }
}
```

### 3.3 缓存精度分析

| 缓存项 | 粒度 | 生命周期 | 收益 |
|--------|------|---------|------|
| `TreeSitterConfig` | 每种语言一个 | 服务进程生命周期 | **最大收益**：避免 70+ 次 query 编译 |
| `ts::Parser` | 每种语言一个（或 pool） | 服务进程生命周期 | 中收益：避免 parser 构造 |
| `Language` 检测结果 | 按路径模式 | 按需 | 低收益：检测本身很快 |

**关键限制：** `TreeSitterConfig` 包含 `ts::Query`，而 `ts::Query` 的 `new()` 编译成本较高。同时 `ts::Language` 是 `Send + Sync`，`TreeSitterConfig` 内所有字段都是只读的，**跨请求完全可安全共享**。

**注意：** `ts::Parser` 不是 `Send`，需要 `tokio::sync::Mutex` 保护或使用连接池模式。

### 3.4 关于 tree-sitter parser 缓存的特殊说明

当前 `to_tree()` 每次创建新的 `ts::Parser`：

```rust
pub(crate) fn to_tree(src: &str, config: &TreeSitterConfig) -> tree_sitter::Tree {
    let mut parser = ts::Parser::new();       // <-- 每次都 new
    parser.set_language(&config.language);
    parser.parse(src, None).unwrap()
}
```

在服务模式下，应为每个语言缓存/池化 `ts::Parser`。由于 `ts::Parser` 不是 `Send`，需要用 `tokio::sync::Mutex` 包装：

```rust
use tokio::sync::Mutex;

struct ParserPool {
    // 每种语言一个 Mutex 保护的 parser
    parsers: DftHashMap<Language, Mutex<ts::Parser>>,
}

impl ParserPool {
    fn get_or_create(&mut self, language: Language, config: &TreeSitterConfig) -> &Mutex<ts::Parser> {
        self.parsers.entry(language)
            .or_insert_with(|| {
                let mut parser = ts::Parser::new();
                parser.set_language(&config.language)
                    .expect("Incompatible tree-sitter version");
                Mutex::new(parser)
            })
    }
}
```

---

## 4. CLI 模块的抽取

### 4.1 `api/cli.rs` 结构

当前 `main.rs` 包含了 CLI 特有的逻辑（参数解析、stdin 读取、文件 I/O、直接打印到终端）。这些逻辑应整体移至 `api/cli.rs`：

```rust
// api/cli.rs
pub(crate) fn run() {
    match options::parse_args() {
        Mode::Diff { diff_options, display_options, ... } => {
            let diff_result = diff_file(...);
            print_diff_result(&display_options, &diff_result);
        }
        Mode::DiffFromConflicts { ... } => { /* ... */ }
        Mode::DumpTreeSitter { ... } => { /* ... */ }
        Mode::DumpSyntax { ... } => { /* ... */ }
        Mode::DumpSyntaxDot { ... } => { /* ... */ }
        Mode::ListLanguages { ... } => { /* ... */ }
        Mode::GitHasUnmergedFile { ... } => { /* ... */ }
    }
}

/// 从 main.rs 移过来，无需改动逻辑
fn diff_file(...) -> DiffResult { /* ... */ }
fn diff_directories(...) -> impl ParallelIterator<Item = DiffResult> { /* ... */ }
fn print_diff_result(...) { /* ... */ }
```

**关键：** `cli` feature 为默认开启，不引入任何额外依赖。

### 4.2 main.rs 精简后

```rust
// main.rs — 仅保留全局初始化 + 模式分发
#![allow(...)] // 保留原有 lint 配置

mod api;
mod conflicts;
mod constants;
mod diff;
mod display;
mod exit_codes;
mod files;
mod gitattributes;
mod hash;
mod line_parser;
mod lines;
mod options;
mod parse;
mod summary;
mod version;
mod words;

#[macro_use]
extern crate log;

#[cfg(not(any(windows, target_os = "illumos", target_os = "freebsd")))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(any(windows, target_os = "illumos", target_os = "freebsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

extern crate pretty_env_logger;

#[cfg(unix)]
fn reset_sigpipe() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
}
#[cfg(not(unix))]
fn reset_sigpipe() {}

fn main() {
    pretty_env_logger::try_init_timed_custom_env("DFT_LOG")
        .expect("The logger has not been previously initialized");
    reset_sigpipe();
    api::run();
}
```

其中 `api/mod.rs` 根据 feature 导出：

```rust
// api/mod.rs
#[cfg(feature = "grpc")]
mod grpc;
#[cfg(feature = "http")]
mod http;
#[cfg(any(feature = "grpc", feature = "http"))]
mod common;

#[cfg(not(any(feature = "grpc", feature = "http")))]
pub(crate) mod cli;
#[cfg(not(any(feature = "grpc", feature = "http")))]
pub(crate) use cli::run;

#[cfg(feature = "grpc")]
pub(crate) use grpc::run;
#[cfg(feature = "http")]
pub(crate) use http::run;
```

---

## 5. HTTP API 设计

### 5.1 REST 端点（`api/http.rs`）

使用 `axum` 框架：

```rust
use axum::{
    extract::State,
    Json, Router,
    routing::post,
};

async fn diff_handler(
    State(service): State<Arc<DiffService>>,
    Json(req): Json<DiffRequest>,
) -> Json<DiffResponse> {
    // 将 DiffRequest 转换为内部调用
    let options = DiffOptions::default();  // 或从 req 解析
    let display_options = DisplayOptions::default();

    let diff_result = service.diff(
        &req.lhs_content,
        &req.rhs_content,
        req.display_path.as_deref().unwrap_or("unknown"),
        None, // language override from req
        &[],
        &options,
        &display_options,
    );

    Json(DiffResponse::from(diff_result))
}

async fn health_handler() -> &'static str {
    "OK"
}

pub(crate) fn serve() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let service = Arc::new(DiffService::new());
        let app = Router::new()
            .route("/api/v1/diff", post(diff_handler))
            .route("/health", axum::routing::get(health_handler))
            .with_state(service);

        let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });
}
```

### 5.2 gRPC proto 定义（`api/proto/difftastic.proto`）

```protobuf
syntax = "proto3";

package difftastic;

service DiffService {
  rpc Diff(DiffRequest) returns (DiffResponse);
  rpc DiffStream(DiffRequest) returns (stream DiffEvent);
  rpc Health(HealthRequest) returns (HealthResponse);
  rpc ListLanguages(ListLanguagesRequest) returns (ListLanguagesResponse);
}

message DiffRequest {
  string lhs_content = 1;
  string rhs_content = 2;
  optional string display_path = 3;
  optional string language_override = 4;
  optional DiffOptions options = 5;
}

message DiffResponse {
  string display_path = 1;
  string file_format = 2;
  bool has_syntactic_changes = 3;
  optional uint64 lhs_byte_len = 4;
  optional uint64 rhs_byte_len = 5;
  repeated Hunk hunks = 6;
}

message Hunk {
  uint32 lhs_start = 1;
  uint32 rhs_start = 2;
  uint32 lhs_count = 3;
  uint32 rhs_count = 4;
  repeated Line lines = 5;
}
```

### 5.3 gRPC 服务端（`api/grpc.rs`）

```rust
tonic::include_proto!("difftastic");

pub(crate) fn serve() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let service = Arc::new(DiffService::new());
        let grpc_service = DiffServiceServer::from_arc(service);
        let reflection = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(proto_descriptor_set())
            .build()
            .unwrap();

        Server::builder()
            .add_service(grpc_service)
            .add_service(reflection)
            .serve("[::]:50051".parse().unwrap())
            .await
            .unwrap();
    });
}
```

---

## 6. 实现步骤

### Phase 1: 基础设施（无行为变化）

1. **Feature 标识符定义**
   - 在 `Cargo.toml` 中添加 `cli`、`http`、`grpc` features。
   - 默认 `default = ["cli"]`，不增加编译时间。

2. **抽取 `api/cli.rs`**
   - 将 `main.rs` 中的 `diff_file()`、`diff_directories()`、`diff_conflicts_file()`、`print_diff_result()` 移至 `api/cli.rs`。
   - `main.rs` 变为薄的转发层：`mod api; api::cli::run();`。
   - 先行提交，验证 CI 无回归。

3. **定义公共类型 `api/common.rs`**
   - `DiffRequest`、`DiffResponse`、`HunkData`、`LineData` 等结构体。
   - 基于 `DiffResult` 但移除 `FileContent`（服务模式不传递原始内容）。
   - 包含 `DiffService` 结构体的骨架。

### Phase 2: 缓存机制

4. **TreeSitterConfig 惰性缓存**
   - `DiffService` 持有 `OnceCell<Mutex<DftHashMap<Language, Arc<TreeSitterConfig>>>>`。
   - `from_language()` 返回值包裹为 `Arc<TreeSitterConfig>`。
   - `to_tree_with_limit` 和 `to_syntax_with_limit` 接收 `&TreeSitterConfig`（而非 `&Arc<TreeSitterConfig>`），内部代码无需改动。

5. **Parser 池化**（可选优化）
   - 在 `DiffService` 中添加 `ParserPool`，按语言缓存 `Mutex<ts::Parser>`。
   - 修改 `to_tree()` 接受 `&Mutex<ts::Parser>` 以避免重复构造。

### Phase 3: 服务端实现

6. **HTTP 服务端 `api/http.rs`**
   - 添加 `axum` + `tokio` 依赖（grpc feature 下）。
   - 实现 `POST /api/v1/diff` 端点。
   - 集成 `DiffService::diff()`。

7. **gRPC 服务端 `api/grpc.rs`**
   - 添加 `tonic` + `tonic-build` 依赖。
   - 定义 `.proto` 文件。
   - 集成 `DiffService::diff()`。

### Phase 4: 优化与测试

8. **基准测试**：对比 CLI 模式 vs HTTP 模式对相同内容的多次调用性能。

9. **并发安全验证**：确保 `Arc<TreeSitterConfig>` 的跨线程共享安全（只读，安全）。

10. **客户端 SDK**：为 HTTP/gRPC 提供示例客户端，便于编辑器集成。

---

## 7. 关键设计决策记录

### 7.1 为什么 `TreeSitterConfig` 可以安全缓存

`TreeSitterConfig` 的字段：
| 字段 | 是否线程安全 | 说明 |
|------|------------|------|
| `language: ts::Language` | `Send + Sync` | 内部是指针的 newtype |
| `atom_nodes: DftHashSet<&'static str>` | 只读，`&str` 为静态生命周期 | 多线程只读安全 |
| `delimiter_tokens: Vec<(&str, &str)>` | 同上 | 同上 |
| `highlight_query: ts::Query` | `Send + Sync` | tree-sitter query 是线程安全的 |
| `sub_languages: Vec<TreeSitterSubLanguage>` | 含 `ts::Query`，`Send + Sync` | 同上 |

**结论：** `Arc<TreeSitterConfig>` 可以安全地在多线程间共享。

### 7.2 为什么 `ts::Parser` 需要 Mutex 保护

tree-sitter 的 `Parser` 内部含有可变状态（如 `cancellation_flag`），在 tree-sitter 文档中明确了 `Parser` 不是 `Send` 的，一次只能被一个线程使用。因此需要用 `Mutex` 包装。

### 7.3 CLI 模式和服务模式的 diff 结果应保持完全一致

`DiffService::diff()` 的 diff 核心逻辑需要与 `cli.rs` 中的 `diff_file_content()` 共享同一套代码路径，避免两种模式产生不同的 diff 结果。推荐的做法是：

- `cli.rs` 中的 `diff_file()` 在读取文件、处理 /dev/null 等 I/O 逻辑后，最终调用 `DiffService::diff()`。
- 两种模式共享同一核心实现。

### 7.4 编译体积

最终可执行文件大小：
- CLI-only: 约当前大小（~20-30MB）
- CLI + server features: 增加 axum/tokio/tonic 依赖，约增加 10-20MB
- 建议 `http` 和 `grpc` 为独立的 feature，用户按需选择
- 也可发布单独的 `difft-server` 二进制包

### 7.5 条件编译的另一种选择

除了 feature flags，也可以使用独立的二进制目标：

```toml
[[bin]]
name = "difft"
path = "src/main.rs"

[[bin]]
name = "difft-server"
path = "src/server_main.rs"
```

`server_main.rs` 可独立引入 tokio、tonic 等依赖，不污染主二进制。但缺点是存在代码重复。**推荐 feature flag 方案**，两种模式的公共代码用 `#[cfg]` 控制。
