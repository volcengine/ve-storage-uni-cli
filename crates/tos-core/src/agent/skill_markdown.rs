//! Portable command skill documents built from registry metadata and CLI syntax.

use clap::{Args, Command, Subcommand};
use serde_json::Value;

use super::{
    global_args::GlobalArgs,
    skill_guide::{auth_guide, command_guide},
};

/// Registry metadata needed to render a command skill.
///
/// `command` is the canonical registry ID; `public_command` and `examples` have
/// already been resolved against the current dedicated or unified entrypoint.
pub struct CommandSkill<'a> {
    pub name: &'a str,
    pub command: &'a str,
    pub public_command: &'a str,
    pub description: &'a str,
    pub risk: &'a str,
    pub schema: &'a Value,
    pub examples: &'a [String],
    pub is_chinese: bool,
}

/// Render YAML frontmatter for a command skill or index.
///
/// Parameters are registry-owned names and descriptions. Returns escaped YAML
/// with a hyphenated skill name. This function performs no IO and cannot fail.
pub fn frontmatter(name: &str, description: &str) -> String {
    // [Review Fix #Skill1] JSON string quoting is valid YAML and prevents a
    // description containing colons or newlines from corrupting frontmatter.
    let description = Value::String(description.to_string());
    format!(
        "---\nname: {}\ndescription: {description}\n---\n\n",
        name.replace('_', "-")
    )
}

/// Build the command tree for parser `S` once for a complete export.
///
/// Returns built CLI syntax including global arguments; performs no IO.
/// Panics only if the developer-defined clap command tree is invalid.
pub fn command_tree<S: Subcommand>() -> Command {
    let mut root = S::augment_subcommands(GlobalArgs::augment_args(Command::new("cli")));
    root.build();
    root
}

/// Render an installable command skill using a built command tree.
///
/// `skill` supplies localized metadata and entrypoint-aware examples; `root`
/// supplies parser syntax built by `command_tree`. Returns
/// Markdown with CLI argument syntax, operational guidance and the MCP schema.
/// It performs no IO; missing parser entries direct the reader to live help.
pub fn render(skill: &CommandSkill<'_>, root: &Command) -> String {
    let description = if skill.is_chinese {
        format!(
            "当用户需要运行 {} 时使用。{}",
            skill.public_command, skill.description
        )
    } else {
        format!(
            "Use when running {}. {}",
            skill.public_command, skill.description
        )
    };
    let mut document = frontmatter(skill.name, &description);
    let heading = if skill.is_chinese {
        "说明"
    } else {
        "Description"
    };
    document.push_str(&format!(
        "# {}\n\n## {heading}\n\n{}\n\n`{}`\n\n",
        skill.name, skill.description, skill.public_command
    ));
    let risk_label = if skill.is_chinese {
        "风险等级"
    } else {
        "Risk level"
    };
    document.push_str(&format!("{risk_label}: `{}`\n\n", skill.risk));
    document.push_str(&cli_reference(skill, root));
    let (surface, suffix) = skill.command.split_once(' ').unwrap_or((skill.command, ""));
    document.push_str(auth_guide(surface, skill.is_chinese));
    document.push_str(command_guide(surface, suffix, skill.is_chinese));
    document.push('\n');
    append_examples(&mut document, skill);
    append_execution(&mut document, skill);
    append_schema(&mut document, skill);
    document
}

fn append_schema(document: &mut String, skill: &CommandSkill<'_>) {
    let heading = if skill.is_chinese {
        "MCP 输入契约"
    } else {
        "MCP input contract"
    };
    let note = if skill.is_chinese {
        "以下为 MCP 参数，不是可直接照搬的 CLI flags。`execute` 是 MCP 控制字段；普通 CLI 调用无需此字段。"
    } else {
        "These are MCP arguments, not literal CLI flags. `execute` controls MCP execution; normal CLI calls do not take that field."
    };
    document.push_str(&format!(
        "## {heading}\n\n{note}\n\n```json\n{:#}\n```\n",
        skill.schema
    ));
}

fn cli_reference(skill: &CommandSkill<'_>, root: &Command) -> String {
    let mut selected = root;
    for component in skill.command.split_whitespace().skip(1) {
        let Some(child) = selected.find_subcommand(component) else {
            return format!("`{} --help`\n\n", skill.public_command);
        };
        selected = child;
    }
    let mut command = selected.clone();
    let heading = if skill.is_chinese {
        "CLI 用法与参数"
    } else {
        "CLI usage and parameters"
    };
    let columns = if skill.is_chinese {
        "| 参数 | 必填 | 可选值 / 默认值 | 说明 |\n|---|---|---|---|\n"
    } else {
        "| Argument | Required | Choices / defaults | Description |\n|---|---|---|---|\n"
    };
    let mut reference = format!(
        "## {heading}\n\n```text\n{}\n```\n\n{columns}",
        public_usage(&mut command, skill)
    );
    for argument in command
        .get_arguments()
        .filter(|argument| is_visible_argument(argument, skill))
    {
        reference.push_str(&argument_row(argument, skill));
    }
    for child in command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
    {
        reference.push_str(&format!(
            "\n- `{} {} --help`\n",
            skill.public_command,
            child.get_name()
        ));
    }
    reference.push('\n');
    reference
}

fn argument_row(argument: &clap::Arg, skill: &CommandSkill<'_>) -> String {
    let syntax = argument_syntax(argument);
    let description = argument_description(argument, skill);
    let choices = argument
        .get_possible_values()
        .iter()
        .map(|value| value.get_name().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let defaults = argument
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ");
    let required = match (skill.is_chinese, argument.is_required_set()) {
        (true, true) => "是",
        (true, false) => "否",
        (false, true) => "yes",
        (false, false) => "no",
    };
    format!(
        "| `{syntax}` | {required} | {} / {} | {} |\n",
        cell(&choices),
        cell(&defaults),
        cell(&description)
    )
}

fn public_usage(command: &mut Command, skill: &CommandSkill<'_>) -> String {
    // [Review Fix #Skill4] clap caches usage_name when building the tree;
    // setting bin_name afterward does not change the rendered usage line.
    let suffix = skill
        .command
        .split_once(' ')
        .map(|(_, suffix)| suffix)
        .unwrap_or("");
    command
        .render_usage()
        .to_string()
        .replacen(&format!("cli {suffix}"), skill.public_command, 1)
}

fn is_visible_argument(argument: &clap::Arg, skill: &CommandSkill<'_>) -> bool {
    if argument.is_hide_set() {
        return false;
    }
    match argument.get_id().as_str() {
        // [Review Fix #Skill7] ByteTOS reuses CpArgs but rejects storage-class;
        // match the public help surface rather than advertising a rejected flag.
        "storage_class" if skill.command.starts_with("tos ") => false,
        "psm" | "idc" | "cluster" | "addr_family" => skill.command.starts_with("tos "),
        "control_endpoint" | "account_id" => skill.command.starts_with("ve-tos "),
        _ => true,
    }
}

fn argument_syntax(argument: &clap::Arg) -> String {
    let identifier = argument.get_id().as_str();
    let Some(long) = argument.get_long() else {
        return format!("<{identifier}>");
    };
    if argument.get_action().takes_values() {
        format!("--{long} <{}>", identifier.to_uppercase())
    } else {
        format!("--{long}")
    }
}

fn argument_description(argument: &clap::Arg, skill: &CommandSkill<'_>) -> String {
    // [Review Fix #Skill5] Registries use both dashed CLI names and Rust IDs.
    let properties = &skill.schema["properties"];
    let schema = properties
        .get(argument.get_id().as_str())
        .or_else(|| argument.get_long().and_then(|long| properties.get(long)));
    if let Some(description) = schema.and_then(|schema| schema["description"].as_str()) {
        return description.to_string();
    }
    if skill.is_chinese {
        format!("参见 `{} --help`", skill.public_command)
    } else {
        argument
            .get_long_help()
            .or_else(|| argument.get_help())
            .map(ToString::to_string)
            .unwrap_or_default()
    }
}

fn cell(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace('\n', "<br>")
}

fn append_examples(document: &mut String, skill: &CommandSkill<'_>) {
    let heading = if skill.is_chinese {
        "示例"
    } else {
        "Examples"
    };
    document.push_str(&format!("## {heading}\n\n"));
    if skill.examples.is_empty() {
        document.push_str(&format!(
            "```bash\n{} --help\n```\n\n",
            skill.public_command
        ));
    }
    for example in skill.examples {
        document.push_str(&format!("```bash\n{example}\n```\n\n"));
    }
    if skill.command.ends_with(" cp") {
        append_copy_examples(document, skill);
    }
}

fn append_copy_examples(document: &mut String, skill: &CommandSkill<'_>) {
    let remote = if skill.command.starts_with("ve-adrive ") {
        "adrive://instance-id/space-id"
    } else {
        "tos://bucket"
    };
    let command = skill.public_command;
    for arguments in [
        format!("./file.txt {remote}/file.txt"),
        format!("{remote}/file.txt ./download.txt"),
        format!("{remote}/file.txt {remote}/copy.txt"),
        format!("./source/ {remote}/backup/ --recursive --include-parent"),
        format!("{remote}/prefix/ ./download/ --recursive --include '*.log'"),
    ] {
        document.push_str(&format!(
            "```bash\n{command} {arguments} --dry-run --output json\n```\n\n"
        ));
    }
}

fn append_execution(document: &mut String, skill: &CommandSkill<'_>) {
    let command = skill.public_command;
    let text = if skill.is_chinese {
        format!("## 执行建议与验证\n\n先运行 `{command} --help` 或 `{command} --describe` 检查当前版本契约。按需补全必填参数。写入或删除前，在支持时使用 `--dry-run --output json` 核对计划；dry-run 不代表远端写入成功。只在用户授权范围内执行，按具体操作保留必需的 `--force` 和精确匹配目标的 `--confirm`。\n\n读取进程退出码和 JSON 结果，区分计划、成功、失败及部分成功；批量任务核对失败条目或报告，再用只读命令验证目标。保留 request_id 便于排错，日志不包含凭据或签名 URL。\n\n")
    } else {
        format!("## Execution and verification\n\nInspect `{command} --help` or `{command} --describe` for this version's contract; supply required arguments as needed. Before writes or deletes, use `--dry-run --output json` where supported to inspect the plan; a dry run does not prove a remote write succeeded. Execute only within the user's authorization and preserve operation-specific required `--force` and exact `--confirm` targets.\n\nRead the process exit code and JSON result. Distinguish plans, success, failure and partial success; inspect failed items or reports for batches, then verify the destination with read-only commands. Retain request_id for diagnosis without logging credentials or signed URLs.\n\n")
    };
    document.push_str(&text);
}
