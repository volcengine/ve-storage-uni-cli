//! Presentation of CLI-authored command instructions, separate from machine IDs.

/// Service-independent fallback until an error is rendered by a CLI entrypoint.
pub const UNKNOWN_ERROR_ACTION: &str =
    "inspect status_code, ec, and request_id; run the current CLI doctor if needed";

// Presentation vocabulary only; a root-tree regression guards new commands.
const COMMAND_ROOTS: &[&str] = &[
    "accelerator",
    "access-monitor",
    "acl",
    "ap",
    "api",
    // [Review Fix #4] Keep assistant command help and parse recovery in the presentation vocabulary.
    "ask",
    "auth",
    "bucket",
    "cap",
    "capabilities",
    "cat",
    "cdn-notification",
    "chunk",
    "completion",
    "config",
    "control",
    "cors",
    "cp",
    "crt",
    "custom-domain",
    "data-process",
    "dataset",
    "del",
    "doctor",
    "du",
    "encryption",
    "file",
    "find",
    "help",
    "https-config",
    "intelligent-tiering",
    "inventory",
    "lifecycle",
    "logging",
    "ls",
    "max-age",
    "mb",
    "mirror",
    "mkdir",
    "mrap",
    "multipart",
    "mv",
    "notification",
    "object",
    "object-set",
    "pay-by-traffic",
    "payment",
    "policy",
    "presign",
    "put",
    "quota",
    "rb",
    "real-time-log",
    "redundancy-transition",
    "rename",
    "replication",
    "restore",
    "rm",
    "serve",
    "share",
    "skill",
    "stat",
    "storageclass",
    "sync",
    "tagging",
    "transfer-acceleration",
    "trash",
    "turbo",
    "versioning",
    "website",
    "worm",
];

/// Whether a token is a registered command root used in human instructions.
/// This presentation check does not parse, authorize, or execute a command.
pub fn is_command_root(token: &str) -> bool {
    COMMAND_ROOTS.contains(&token)
}

/// Resolve the shared unknown-error action for `surface`, preserving other actions.
/// Returns a display-only instruction without inspecting or rewriting error messages.
pub fn diagnostic_action(action: &str, surface: &str) -> String {
    if action == UNKNOWN_ERROR_ACTION {
        format!(
            "inspect status_code, ec, and request_id; run {} doctor if needed",
            invocation_prefix(surface)
        )
    } else {
        action.to_string()
    }
}

/// Resolve a service's invocation prefix from entrypoint-owned configuration.
/// `surface` is `tos`, `ve-tos`, or `ve-adrive`; unknown names pass through.
/// Returns the canonical service name when no entrypoint has configured a prefix.
pub fn invocation_prefix(surface: &str) -> String {
    let variable = match surface {
        "tos" => "VE_STORAGE_UNI_BYTED_TOS_EXAMPLE_PREFIX",
        "ve-tos" => "VE_STORAGE_UNI_TOS_EXAMPLE_PREFIX",
        "ve-adrive" => "VE_STORAGE_UNI_ADRIVE_EXAMPLE_PREFIX",
        _ => return surface.to_string(),
    };
    std::env::var(variable).unwrap_or_else(|_| surface.to_string())
}

/// Render commands embedded in a trusted CLI-authored template for `surface`.
/// Returns display text; does not execute commands or change machine identifiers.
/// Never pass upstream text, user input, filesystem paths, or serialized payloads.
pub fn render_template(template: &str, surface: &str) -> String {
    render_template_with_prefix(template, surface, &invocation_prefix(surface))
}

/// Render trusted instructions using an explicit invocation `prefix`.
/// `surface` identifies source command spellings, allowing shared TOS templates
/// to target the active TOS surface. Returns text without recursively replacing
/// inserted prefixes. User data must be interpolated only after this operation.
pub fn render_template_with_prefix(template: &str, surface: &str, prefix: &str) -> String {
    let aliases = command_aliases(surface);
    if aliases.iter().any(|alias| template == alias) {
        return prefix.to_string();
    }
    let mut rendered = String::with_capacity(template.len());
    let mut copied_until = 0;
    // [Review Fix #CommandDisplay1] One pass prevents duplicate prefixes;
    // token boundaries keep configuration keys and completion filenames intact.
    for (offset, _) in template.char_indices() {
        if offset < copied_until || !is_command_boundary(&template[..offset]) {
            continue;
        }
        let Some(alias) = aliases.iter().find(|alias| {
            template[offset..]
                .strip_prefix(alias.as_str())
                .is_some_and(is_command_suffix)
        }) else {
            continue;
        };
        rendered.push_str(&template[copied_until..offset]);
        rendered.push_str(prefix);
        copied_until = offset + alias.len();
    }
    rendered.push_str(&template[copied_until..]);
    rendered
}

/// Render trusted help prose and direct/pipeline examples, preserving arguments.
/// Uses `surface` spellings with `prefix`; shell selectors, quoted data and query
/// values remain untouched while executable positions follow the entrypoint.
pub fn render_help_template(help: &str, surface: &str, prefix: &str) -> String {
    let aliases = command_aliases(surface);
    let mut is_annotation_open = false;
    help.split_inclusive('\n')
        .map(|line| {
            // [Review Fix #CommandDisplay10] Clap annotations can contain
            // real environment/default values, not CLI-authored instructions.
            if is_annotation_open {
                is_annotation_open = !line.contains(']');
                return line.to_string();
            }
            let annotation_start = [
                "[env:",
                "[default:",
                "[possible values:",
                "[环境变量:",
                "[默认:",
                "[可选值:",
            ]
            .iter()
            .filter_map(|marker| line.find(marker))
            .min()
            .unwrap_or(line.len());
            let (prose, annotation) = line.split_at(annotation_start);
            is_annotation_open = !annotation.is_empty() && !annotation.contains(']');
            let trimmed = prose.trim_start();
            let is_shell = aliases
                .iter()
                .any(|alias| trimmed.starts_with(&format!("{alias} ")))
                || ["mkdir ", "echo ", "printf ", "cat ", "source ", "eval "]
                    .iter()
                    .any(|start| trimmed.starts_with(start));
            let mut rendered = if is_shell {
                render_shell_line(prose, &aliases, prefix)
            } else {
                render_template_with_prefix(prose, surface, prefix)
            };
            rendered.push_str(annotation);
            rendered
        })
        .collect()
}

fn command_aliases(surface: &str) -> [String; 3] {
    [
        format!("ve-storage-uni-cli {surface}"),
        format!("{surface}-cli"),
        surface.to_string(),
    ]
}

fn render_shell_line(line: &str, aliases: &[String], prefix: &str) -> String {
    let mut rendered = String::with_capacity(line.len());
    let (mut copied_until, mut quote, mut is_escaped, mut is_command) = (0, None, false, true);
    for (offset, character) in line.char_indices() {
        if offset < copied_until {
            continue;
        }
        if consume_shell_syntax(character, &mut quote, &mut is_escaped, &mut is_command) {
            continue;
        }
        if !is_command || character.is_whitespace() {
            continue;
        }
        if let Some(alias) = aliases.iter().find(|alias| {
            line[offset..]
                .strip_prefix(alias.as_str())
                .is_some_and(is_command_suffix)
        }) {
            rendered.push_str(&line[copied_until..offset]);
            rendered.push_str(prefix);
            copied_until = offset + alias.len();
        }
        is_command = false;
    }
    rendered.push_str(&line[copied_until..]);
    rendered
}

fn consume_shell_syntax(
    character: char,
    quote: &mut Option<char>,
    is_escaped: &mut bool,
    is_command: &mut bool,
) -> bool {
    if *is_escaped {
        *is_escaped = false;
    } else if character == '\\' && *quote != Some('\'') {
        *is_escaped = true;
    } else if *quote == Some(character) {
        *quote = None;
    } else if quote.is_some() {
        return true;
    } else if matches!(character, '\'' | '"') {
        *quote = Some(character);
        *is_command = false;
    } else if matches!(character, '|' | '&' | ';') {
        *is_command = true;
    } else {
        return false;
    }
    true
}

fn is_command_boundary(before: &str) -> bool {
    before.chars().next_back().is_none_or(|character| {
        !character.is_ascii_alphanumeric()
            && !matches!(character, '-' | '_' | '/' | '\\' | '.' | ':' | '@')
    })
}

fn is_command_suffix(after: &str) -> bool {
    if !after.starts_with(char::is_whitespace) {
        return false;
    }
    let suffix = after.trim_start();
    let root = suffix
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_')
        })
        .next()
        .unwrap_or_default();
    // [Review Fix #CommandDisplay5] Product names before prose such as
    // "ve-tos credentials" are not executable command references.
    is_command_root(root) || suffix.starts_with(['-', '<', '['])
}

#[cfg(test)]
mod tests {
    use super::render_template_with_prefix as render;

    #[test]
    fn renders_known_commands_in_prose_and_pipelines() {
        let template = "run `ve-adrive auth login`; ve-adrive cat a | ve-adrive-cli put b";
        assert_eq!(
            render(template, "ve-adrive", "ve-adrive-cli"),
            "run `ve-adrive-cli auth login`; ve-adrive-cli cat a | ve-adrive-cli put b"
        );
    }

    #[test]
    fn unified_rendering_is_idempotent() {
        let prefix = "ve-storage-uni-cli ve-tos";
        let template = "Use ve-tos-cli doctor, or ve-storage-uni-cli ve-tos config show";
        let once = render(template, "ve-tos", prefix);
        assert_eq!(
            once,
            "Use ve-storage-uni-cli ve-tos doctor, or ve-storage-uni-cli ve-tos config show"
        );
        assert_eq!(render(&once, "ve-tos", prefix), once);
    }

    #[test]
    fn preserves_namespaces_paths_and_external_commands() {
        let template = "default.ve-tos.endpoint /tmp/ve-tos doctor ve login; tos-cli.fish";
        assert_eq!(render(template, "ve-tos", "ve-tos-cli"), template);
    }

    #[test]
    fn shared_templates_can_target_bytetos() {
        assert_eq!(
            render("use ve-tos config --help", "ve-tos", "tos-cli"),
            "use tos-cli config --help"
        );
    }

    #[test]
    fn help_preserves_canonical_selectors_and_literal_shell_data() {
        let source = "  ve-tos-cli skill export --name 've-tos bucket create'\n  echo 've-tos cp' | ve-tos-cli put tos://bucket/key\n";
        let rendered = super::render_help_template(source, "ve-tos", "ve-storage-uni-cli ve-tos");
        assert_eq!(rendered, "  ve-storage-uni-cli ve-tos skill export --name 've-tos bucket create'\n  echo 've-tos cp' | ve-storage-uni-cli ve-tos put tos://bucket/key\n");
    }

    #[test]
    fn help_preserves_user_environment_and_schema_annotations() {
        let source = "run ve-tos doctor [env: TOS_PROFILE=ve-tos doctor]\n[默认: ve-tos cp]\n[possible values: ve-tos rm]\n";
        assert_eq!(super::render_help_template(source, "ve-tos", "ve-tos-cli"),
            "run ve-tos-cli doctor [env: TOS_PROFILE=ve-tos doctor]\n[默认: ve-tos cp]\n[possible values: ve-tos rm]\n");
    }

    #[test]
    fn preserves_other_services_and_plain_product_names() {
        let template = "ve-adrive-cli / ve-adrive; ve-adrive credentials; run ve-tos doctor";
        assert_eq!(
            render(template, "ve-adrive", "ve-storage-uni-cli ve-adrive"),
            template
        );
    }
}
