use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::diff::changes::ChangeMap;
use crate::diff::dijkstra::ExceededGraphLimit;
use crate::diff::{dijkstra, unchanged};
use crate::display::context::opposite_positions;
use crate::display::hunks::{matched_pos_to_hunks, merge_adjacent};
use crate::lines::MaxLine;
use crate::options::{DiffOptions, DisplayOptions};
use crate::parse::guess_language::{self, Language, LanguageOverride};
use crate::parse::syntax;
use crate::parse::syntax::init_next_prev;
use crate::parse::tree_sitter_parser::{self as tsp, TreeSitterConfig};
use crate::summary::{DiffResult, FileContent, FileFormat};
use typed_arena::Arena;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffRequest {
    pub lhs_content: String,
    pub rhs_content: String,
    pub display_path: Option<String>,
    pub language_override: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffResponse {
    pub display_path: String,
    pub file_format: String,
    pub has_syntactic_changes: bool,
    pub has_byte_changes: bool,
    pub lhs_byte_len: Option<usize>,
    pub rhs_byte_len: Option<usize>,
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

pub struct DiffService {
    config_cache: Mutex<Vec<(Language, Arc<TreeSitterConfig>)>>,
}

impl DiffService {
    pub fn new() -> Self {
        Self {
            config_cache: Mutex::new(Vec::new()),
        }
    }

    fn get_config(&self, language: Language) -> Arc<TreeSitterConfig> {
        let mut map = self.config_cache.lock().unwrap();
        if let Some((_, config)) = map.iter().find(|(lang, _)| *lang == language) {
            return config.clone();
        }
        let config = Arc::new(tsp::from_language(language));
        map.push((language, config.clone()));
        config
    }

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
        let guess_src = rhs_content;
        let language = language_override
            .or_else(|| guess_language::guess(Path::new(display_path), guess_src, overrides));

        if lhs_content == rhs_content {
            let file_format = match language {
                Some(language) => FileFormat::SupportedLanguage(language),
                None => FileFormat::PlainText,
            };

            return DiffResult {
                extra_info: None,
                display_path: display_path.to_owned(),
                file_format,
                lhs_src: FileContent::Text("".into()),
                rhs_src: FileContent::Text("".into()),
                lhs_positions: vec![],
                rhs_positions: vec![],
                hunks: vec![],
                has_byte_changes: None,
                has_syntactic_changes: false,
            };
        }

        let lang_config = language.map(|lang| (lang, self.get_config(lang)));

        let (file_format, lhs_positions, rhs_positions) = match lang_config {
            None => {
                let file_format = FileFormat::PlainText;
                if diff_options.check_only {
                    return check_only_text(
                        &file_format,
                        display_path,
                        None,
                        lhs_content,
                        rhs_content,
                    );
                }

                let lhs_positions = crate::line_parser::change_positions(lhs_content, rhs_content);
                let rhs_positions = crate::line_parser::change_positions(rhs_content, lhs_content);
                (file_format, lhs_positions, rhs_positions)
            }
            Some((language, lang_config)) => {
                let arena = Arena::new();
                match tsp::to_tree_with_limit(diff_options, &lang_config, lhs_content, rhs_content)
                {
                    Ok((lhs_tree, rhs_tree)) => {
                        match tsp::to_syntax_with_limit(
                            lhs_content,
                            rhs_content,
                            &lhs_tree,
                            &rhs_tree,
                            &arena,
                            &lang_config,
                            diff_options,
                        ) {
                            Ok((lhs, rhs)) => {
                                if diff_options.check_only {
                                    let has_syntactic_changes = lhs != rhs;
                                    let has_byte_changes = if lhs_content == rhs_content {
                                        None
                                    } else {
                                        Some((
                                            lhs_content.as_bytes().len(),
                                            rhs_content.as_bytes().len(),
                                        ))
                                    };

                                    return DiffResult {
                                        extra_info: None,
                                        display_path: display_path.to_owned(),
                                        file_format: FileFormat::SupportedLanguage(language),
                                        lhs_src: FileContent::Text(lhs_content.to_owned()),
                                        rhs_src: FileContent::Text(rhs_content.to_owned()),
                                        lhs_positions: vec![],
                                        rhs_positions: vec![],
                                        hunks: vec![],
                                        has_byte_changes,
                                        has_syntactic_changes,
                                    };
                                }

                                let mut change_map = ChangeMap::default();
                                let possibly_changed =
                                    if std::env::var("DFT_DBG_KEEP_UNCHANGED").is_ok() {
                                        vec![(lhs.clone(), rhs.clone())]
                                    } else {
                                        unchanged::mark_unchanged(&lhs, &rhs, &mut change_map)
                                    };

                                let mut exceeded_graph_limit = false;

                                for (lhs_section_nodes, rhs_section_nodes) in possibly_changed {
                                    init_next_prev(&lhs_section_nodes);
                                    init_next_prev(&rhs_section_nodes);

                                    match dijkstra::mark_syntax(
                                        lhs_section_nodes.first().copied(),
                                        rhs_section_nodes.first().copied(),
                                        &mut change_map,
                                        diff_options.graph_limit,
                                    ) {
                                        Ok(()) => {}
                                        Err(ExceededGraphLimit {}) => {
                                            exceeded_graph_limit = true;
                                            break;
                                        }
                                    }
                                }

                                if exceeded_graph_limit {
                                    let lhs_positions = crate::line_parser::change_positions(
                                        lhs_content,
                                        rhs_content,
                                    );
                                    let rhs_positions = crate::line_parser::change_positions(
                                        rhs_content,
                                        lhs_content,
                                    );
                                    (
                                        FileFormat::TextFallback {
                                            reason: "exceeded DFT_GRAPH_LIMIT".into(),
                                        },
                                        lhs_positions,
                                        rhs_positions,
                                    )
                                } else {
                                    crate::diff::sliders::fix_all_sliders(
                                        language,
                                        &lhs,
                                        &mut change_map,
                                    );
                                    crate::diff::sliders::fix_all_sliders(
                                        language,
                                        &rhs,
                                        &mut change_map,
                                    );

                                    let mut lhs_positions =
                                        syntax::change_positions(&lhs, &change_map);
                                    let mut rhs_positions =
                                        syntax::change_positions(&rhs, &change_map);

                                    if diff_options.ignore_comments {
                                        let lhs_comments = tsp::comment_positions(
                                            &lhs_tree,
                                            lhs_content,
                                            &lang_config,
                                        );
                                        lhs_positions.extend(lhs_comments);

                                        let rhs_comments = tsp::comment_positions(
                                            &rhs_tree,
                                            rhs_content,
                                            &lang_config,
                                        );
                                        rhs_positions.extend(rhs_comments);
                                    }

                                    (
                                        FileFormat::SupportedLanguage(language),
                                        lhs_positions,
                                        rhs_positions,
                                    )
                                }
                            }
                            Err(tsp::ExceededParseErrorLimit(error_count)) => {
                                let file_format = FileFormat::TextFallback {
                                    reason: format!(
                                        "{} {} parse error{}, exceeded DFT_PARSE_ERROR_LIMIT",
                                        error_count,
                                        guess_language::language_name(language),
                                        if error_count == 1 { "" } else { "s" }
                                    ),
                                };

                                if diff_options.check_only {
                                    return check_only_text(
                                        &file_format,
                                        display_path,
                                        None,
                                        lhs_content,
                                        rhs_content,
                                    );
                                }

                                let lhs_positions = crate::line_parser::change_positions(
                                    lhs_content,
                                    rhs_content,
                                );
                                let rhs_positions = crate::line_parser::change_positions(
                                    rhs_content,
                                    lhs_content,
                                );
                                (file_format, lhs_positions, rhs_positions)
                            }
                        }
                    }
                    Err(tsp::ExceededByteLimit(num_bytes)) => {
                        use humansize::{format_size, FormatSizeOptions, BINARY};
                        let format_options = FormatSizeOptions::from(BINARY).decimal_places(1);
                        let file_format = FileFormat::TextFallback {
                            reason: format!(
                                "{} exceeded DFT_BYTE_LIMIT",
                                &format_size(num_bytes, format_options)
                            ),
                        };

                        if diff_options.check_only {
                            return check_only_text(
                                &file_format,
                                display_path,
                                None,
                                lhs_content,
                                rhs_content,
                            );
                        }

                        let lhs_positions = crate::line_parser::change_positions(
                            lhs_content,
                            rhs_content,
                        );
                        let rhs_positions = crate::line_parser::change_positions(
                            rhs_content,
                            lhs_content,
                        );
                        (file_format, lhs_positions, rhs_positions)
                    }
                }
            }
        };

        let opposite_to_lhs = opposite_positions(&lhs_positions);
        let opposite_to_rhs = opposite_positions(&rhs_positions);

        let hunks = matched_pos_to_hunks(&lhs_positions, &rhs_positions);
        let hunks = merge_adjacent(
            &hunks,
            &opposite_to_lhs,
            &opposite_to_rhs,
            lhs_content.max_line(),
            rhs_content.max_line(),
            display_options.num_context_lines as usize,
        );
        let has_syntactic_changes = !hunks.is_empty();

        let has_byte_changes = if lhs_content == rhs_content {
            None
        } else {
            Some((lhs_content.as_bytes().len(), rhs_content.as_bytes().len()))
        };

        DiffResult {
            extra_info: None,
            display_path: display_path.to_owned(),
            file_format,
            lhs_src: FileContent::Text(lhs_content.to_owned()),
            rhs_src: FileContent::Text(rhs_content.to_owned()),
            lhs_positions,
            rhs_positions,
            hunks,
            has_byte_changes,
            has_syntactic_changes,
        }
    }
}

fn check_only_text(
    file_format: &FileFormat,
    display_path: &str,
    extra_info: Option<String>,
    lhs_src: &str,
    rhs_src: &str,
) -> DiffResult {
    let has_byte_changes = if lhs_src == rhs_src {
        None
    } else {
        Some((lhs_src.as_bytes().len(), rhs_src.as_bytes().len()))
    };

    DiffResult {
        display_path: display_path.to_owned(),
        extra_info,
        file_format: file_format.clone(),
        lhs_src: FileContent::Text(lhs_src.to_owned()),
        rhs_src: FileContent::Text(rhs_src.to_owned()),
        lhs_positions: vec![],
        rhs_positions: vec![],
        hunks: vec![],
        has_byte_changes,
        has_syntactic_changes: lhs_src != rhs_src,
    }
}
