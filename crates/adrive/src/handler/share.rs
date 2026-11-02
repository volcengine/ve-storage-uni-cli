//! Share management and receiver dispatch.
use super::common::{
    build_adrive_runtime, map_ids_error, output_result, resolve_target, ParsedADriveUri,
};
use crate::cli::{share::*, ADriveAuthArgs};
use crate::domain::{
    client::{Client, Error},
    share_target::{resolve_share_instance, resolve_share_target, ShareTarget},
    share_types::*,
    share_url::{build_share_url, parse_share_url, validate_share_id_component},
};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tos_core::agent::{envelope::Envelope, error::CliError, global_args::GlobalArgs};
use tos_core::infra::config::ConfigFile;

/// Execute one share action with normal Envelope, query and request-trace behavior.
/// Describe performs no input I/O; download dry-run reads metadata and other plans validate explicit password stdin.
/// Receivers use OAuth only when explicitly selected.
pub async fn handle_share_command(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    group: &ShareCommand,
) -> Result<i32, CliError> {
    let Some(action) = &group.action else {
        if global.describe {
            output_result(
                global,
                &Envelope::success(
                    "ve-adrive share",
                    super::meta::describe_adrive_command_metadata("ve-adrive share"),
                ),
            )?;
            return Ok(0);
        }
        return show_group_help();
    };
    let command = format!("ve-adrive share {}", action.name());
    if global.describe {
        output_result(
            global,
            &Envelope::success(
                &command,
                super::meta::describe_adrive_command_metadata(&command),
            ),
        )?;
        return Ok(0);
    }
    validate_action(action)?;
    if global.dry_run {
        if let ShareAction::Download(args) = action {
            return super::share_download_plan::dry_run(global, auth, args).await;
        }
        validate_dry_run_password(action).await?;
        output_result(global, &Envelope::success(&command, share_plan(action)?))?;
        return Ok(0);
    }
    if let ShareAction::View(args) = action {
        super::share_view::validate(global, args)?;
        if args.is_raw {
            return super::share_view::view(global, auth, args).await;
        }
    }
    // [Review Fix #ShareInterrupt1] Download owns its later aggregate/checkpoint cancellation controller.
    if let ShareAction::Download(args) = action {
        return super::share_download::download(global, auth, args).await;
    }
    execute_with_interrupt(global, auth, action).await
}

async fn validate_dry_run_password(action: &ShareAction) -> Result<(), CliError> {
    let is_password_stdin = match action {
        ShareAction::Create(args) => args.settings.is_password_stdin,
        ShareAction::Update(args) => args.settings.is_password_stdin,
        _ => false,
    };
    if is_password_stdin {
        // [Review Fix #D05] Validate the explicitly selected secret without exposing it in the dry-run plan.
        super::share_password::read_password_stdin().await?;
    }
    Ok(())
}

async fn execute_with_interrupt(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    action: &ShareAction,
) -> Result<i32, CliError> {
    let is_submitted = Arc::new(AtomicBool::new(false));
    // [Review Fix #ShareInterrupt1] Keep the listener alive through password input and subsequent metadata; dropping the action restores EchoGuard.
    tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(CliError::Io)?;
            Err(interrupted_action_error(action, is_submitted.load(Ordering::SeqCst)))
        }
        result = execute_action(global, auth, action, &is_submitted) => result,
    }
}

async fn execute_action(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    action: &ShareAction,
    is_submitted: &Arc<AtomicBool>,
) -> Result<i32, CliError> {
    let command = format!("ve-adrive share {}", action.name());
    match action {
        ShareAction::Inspect(args) => super::share_receive::inspect(global, auth, args).await,
        ShareAction::Files(args) => super::share_receive::files(global, auth, args).await,
        ShareAction::View(args) => super::share_view::view(global, auth, args).await,
        ShareAction::Download(args) => super::share_download::download(global, auth, args).await,
        _ => {
            let result = manage_with_progress(global, auth, action, is_submitted).await?;
            output_result(global, &Envelope::success(command, result))?;
            Ok(0)
        }
    }
}

fn interrupted_action_error(action: &ShareAction, is_submitted: bool) -> CliError {
    if !is_submitted {
        return CliError::Unknown("[share_interrupted] share action interrupted".into());
    }
    let known_id = match action {
        ShareAction::Update(args) => selector_target(&args.selector)
            .ok()
            .map(|target| target.share_id),
        ShareAction::Cancel(args) => selector_target(&cancel_selector(args))
            .ok()
            .map(|target| target.share_id),
        _ => None,
    };
    match known_id {
        Some(id) => unknown_result(&format!("share mutation interrupted after submission for ShareID {id}; reconcile with share get/list before retrying")),
        None => unknown_result("share mutation interrupted after submission; reconcile with share list/get before retrying"),
    }
}

fn show_group_help() -> Result<i32, CliError> {
    use clap::CommandFactory;
    #[derive(clap::Parser)]
    struct GroupHelp {
        #[command(flatten)]
        share: ShareCommand,
    }
    GroupHelp::command()
        .name("ve-adrive-cli share")
        .print_help()?;
    println!();
    Ok(0)
}

fn validate_action(action: &ShareAction) -> Result<(), CliError> {
    match action {
        ShareAction::Create(args) => {
            create_target(args)?;
            validate_settings(&args.settings)?;
            validate_origin(args.share_endpoint.as_deref())
        }
        ShareAction::Get(args) => validate_selector(args),
        ShareAction::Cancel(args) => validate_selector(&cancel_selector(args)),
        ShareAction::Update(args) => {
            validate_selector(&args.selector)?;
            validate_settings(&args.settings)?;
            if !has_changes(&args.settings) {
                return Err(validation(
                    "update requires at least one changed share setting",
                ));
            }
            Ok(())
        }
        ShareAction::List(args) => validate_list(args),
        ShareAction::Inspect(args) => parse_share_url(&args.share_url)
            .map(|_| ())
            .map_err(map_ids_error),
        ShareAction::Files(args) => {
            super::share_listing::validate_arguments(args)?;
            super::high_level::validate_ls_sort(args.sort.as_deref())?;
            parse_share_url(&args.share_url).map_err(map_ids_error)?;
            validate_selection(args.path.as_deref())
        }
        ShareAction::View(args) => {
            parse_share_url(&args.share_url).map_err(map_ids_error)?;
            validate_selection(args.path.as_deref())?;
            if args.path.as_ref().is_some_and(|path| path.ends_with('/')) {
                return Err(validation(
                    "view requires --path to one exact file, not a directory",
                ));
            }
            Ok(())
        }
        ShareAction::Download(args) => {
            super::share_transfer_options::validate(args)?;
            parse_share_url(&args.share_url).map_err(map_ids_error)?;
            validate_selection(args.path.as_deref())?;
            if let Some(path) = args.path.as_deref().filter(|path| *path != "/") {
                super::share_manifest::validate_relative(path.strip_suffix('/').unwrap_or(path))?;
            }
            super::share_content::ContentRuntime::for_args(args)?;
            require_text(&args.destination_dir, "destination directory")
        }
    }
}

fn validate_list(args: &ListShareArgs) -> Result<(), CliError> {
    let instance = list_instance(args)?;
    validate_origin(args.share_endpoint.as_deref())?;
    // A placeholder creator validates all other SDK search controls without resolving identity.
    let mut input = search_input(args, instance);
    if input.creators.is_empty() {
        input.creators.push("validation-only".into());
    }
    input.validate().map_err(map_ids_error)
}
fn list_instance(args: &ListShareArgs) -> Result<String, CliError> {
    resolve_share_instance(args.target.as_deref(), args.instance.as_deref()).map_err(map_ids_error)
}
fn validate_selector(args: &ShareIdArgs) -> Result<(), CliError> {
    selector_target(args)?;
    validate_origin(args.share_endpoint.as_deref())
}
fn selector_target(args: &ShareIdArgs) -> Result<ShareTarget, CliError> {
    resolve_share_target(&args.share_id, args.instance.as_deref()).map_err(map_ids_error)
}
fn validate_origin(origin: Option<&str>) -> Result<(), CliError> {
    if let Some(origin) = origin {
        tos_core::infra::config::validate_share_endpoint(origin)?;
    }
    Ok(())
}
fn require_text(value: &str, label: &str) -> Result<(), CliError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(validation(&format!(
            "{label} must be nonempty and contain no controls"
        )));
    }
    Ok(())
}
/// Validate optional receiver selection without changing the remote path.
/// Returns a validation error for empty or control-containing paths.
pub(crate) fn validate_selection(path: Option<&str>) -> Result<(), CliError> {
    if let Some(path) = path {
        require_text(path, "share path")?;
    }
    Ok(())
}
fn validation(message: &str) -> CliError {
    CliError::ValidationError(message.into())
}
fn create_target(args: &CreateShareArgs) -> Result<ParsedADriveUri, CliError> {
    if args.target.is_some()
        && [&args.instance, &args.space, &args.folder, &args.file]
            .iter()
            .any(|value| value.is_some())
    {
        return Err(validation("URI target conflicts with explicit selectors"));
    }
    // [Review Fix #ShareTarget1] An empty explicit file must not silently become a folder share.
    for (selector, label) in [
        (&args.instance, "instance"),
        (&args.space, "space"),
        (&args.folder, "folder"),
        (&args.file, "file"),
    ] {
        if let Some(value) = selector {
            require_text(value, label)?;
        }
    }
    let mut target = resolve_target(
        args.target.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        args.file.as_deref(),
    )?;
    require_text(&target.instance, "instance")?;
    require_text(&target.space, "space")?;
    // [Review Fix #ShareSpace1] A Space root is an explicit share target, not an empty file selector.
    // [Review Fix #ShareSpace4] Only the exact explicit folder-root selector may normalize `/`; malformed multi-slash paths must never expand sharing scope.
    if target.path == "/"
        && args.target.is_none()
        && args.folder.as_deref() == Some("/")
        && args.file.is_none()
    {
        target.path.clear();
    }
    if !target.path.is_empty() && target.path.bytes().all(|byte| byte == b'/') {
        return Err(validation(
            "Space root must use adrive://instance/space[/] or the exact --folder / selector",
        ));
    }
    if !target.path.is_empty() {
        require_text(&target.path, "file or folder target")?;
    }
    Ok(target)
}

fn create_input(
    args: &CreateShareArgs,
) -> Result<(ParsedADriveUri, CreateShareLinkInput), CliError> {
    let target = create_target(args)?;
    let mut input = settings_input(&args.settings)?;
    input.instance_id = target.instance.clone();
    input.space_id = target.space.clone();
    input.is_share_all_files = target.path.is_empty();
    input.file_path_list = if input.is_share_all_files {
        Vec::new()
    } else {
        vec![target.path.clone()]
    };
    input.expiration_sec = Some(input.expiration_sec.unwrap_or(0));
    input.permission = Some(
        input
            .permission
            .unwrap_or_else(|| DEFAULT_CREATE_SHARE_PERMISSION.into()),
    );
    Ok((target, input))
}
fn validate_settings(settings: &ShareSettings) -> Result<(), CliError> {
    settings_input(settings)?.validate().map_err(map_ids_error)
}
fn has_changes(settings: &ShareSettings) -> bool {
    settings.name.is_some()
        || settings.description.is_some()
        || settings.permission.is_some()
        || settings.expires_in.is_some()
        || settings.is_password_stdin
        || settings.is_login_required.is_some()
        || settings.preview_limit.is_some()
        || settings.download_limit.is_some()
}
fn settings_input(settings: &ShareSettings) -> Result<CreateShareLinkInput, CliError> {
    Ok(CreateShareLinkInput {
        instance_id: "validation".into(),
        space_id: "validation".into(),
        file_path_list: vec!["validation".into()],
        share_name: settings.name.clone(),
        description: settings.description.clone(),
        permission: settings.permission.map(|value| value.as_str().into()),
        expiration_sec: settings
            .expires_in
            .as_deref()
            .map(parse_duration)
            .transpose()?,
        preview_limit: settings.preview_limit,
        download_limit: settings.download_limit,
        require_login: settings.is_login_required,
        ..Default::default()
    })
}
fn parse_duration(value: &str) -> Result<i64, CliError> {
    if value == "0" {
        return Ok(0);
    }
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let number = value[..split].parse::<i64>().map_err(|_| {
        validation("expires-in must be a nonnegative duration such as 0, 3600, 30m, 24h or 7d")
    })?;
    let factor = match &value[split..] {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => return Err(validation("unsupported expires-in duration unit")),
    };
    number
        .checked_mul(factor)
        .ok_or_else(|| validation("expires-in duration exceeds supported range"))
}

#[cfg(test)]
async fn manage(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    action: &ShareAction,
) -> Result<Value, CliError> {
    manage_with_progress(global, auth, action, &Arc::new(AtomicBool::new(false))).await
}
async fn manage_with_progress(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    action: &ShareAction,
    is_submitted: &Arc<AtomicBool>,
) -> Result<Value, CliError> {
    if let ShareAction::Cancel(_) = action {
        confirm_cancel(global).await?;
    }
    let runtime = build_adrive_runtime(global, auth.auth_mode)?;
    // [Review Fix #ShareSubmissionBoundary2] The domain observer marks actual authenticated send attempts, not handler entry.
    let client = runtime
        .client
        .with_share_submission_observer(is_submitted.clone());
    match action {
        ShareAction::Create(args) => manage_create(global, &client, args).await,
        ShareAction::Get(args) => manage_get(global, &client, args).await,
        ShareAction::Update(args) => manage_update(global, &client, args).await,
        ShareAction::Cancel(args) => manage_cancel(&client, args).await,
        ShareAction::List(args) => list_links(global, &client, args).await,
        _ => Err(validation("invalid management action")),
    }
}
async fn manage_create(
    global: &GlobalArgs,
    client: &Client,
    args: &CreateShareArgs,
) -> Result<Value, CliError> {
    let mut resolved = clone_create(args);
    if args.is_by_name {
        let target =
            super::high_level::resolve_parsed_target_by_name(client, create_target(args)?).await?;
        resolved.target = Some(format!(
            "adrive://{}/{}/{}",
            target.instance, target.space, target.path
        ));
        resolved.instance = None;
        resolved.space = None;
        resolved.folder = None;
        resolved.file = None;
    }
    let origin = configured_share_origin(global, resolved.share_endpoint.as_deref())?;
    create_link(client, &resolved, origin.as_deref()).await
}
async fn manage_get(
    global: &GlobalArgs,
    client: &Client,
    args: &ShareIdArgs,
) -> Result<Value, CliError> {
    let selector = resolve_selector(client, args).await?;
    let origin = configured_share_origin(global, args.share_endpoint.as_deref())?;
    get_resolved_link(client, &selector, origin.as_deref()).await
}
async fn manage_update(
    global: &GlobalArgs,
    client: &Client,
    args: &UpdateShareArgs,
) -> Result<Value, CliError> {
    let selector = resolve_selector(client, &args.selector).await?;
    let origin = configured_share_origin(global, args.selector.share_endpoint.as_deref())?;
    update_resolved_link(client, args, &selector, origin.as_deref()).await
}
async fn manage_cancel(client: &Client, args: &CancelShareArgs) -> Result<Value, CliError> {
    // [Review Fix #ShareCancelOrigin1] Revocation needs only API identity, so unrelated page-origin settings cannot block it.
    let selector = resolve_selector(client, &cancel_selector(args)).await?;
    // [Review Fix #ShareUnknown1] Preserve the known selector when cancellation has an ambiguous outcome.
    client
        .cancel_share_link(&CancelShareLinkInput {
            instance_id: selector.instance,
            share_id: selector.share_id.clone(),
        })
        .await
        .map_err(|error| mutation_error_for_id(error, &selector.share_id))?;
    Ok(json!({"ShareID":selector.share_id,"Cancelled":true}))
}
fn cancel_selector(args: &CancelShareArgs) -> ShareIdArgs {
    ShareIdArgs {
        share_id: args.share_id.clone(),
        instance: args.instance.clone(),
        is_by_name: args.is_by_name,
        share_endpoint: None,
    }
}
fn clone_create(args: &CreateShareArgs) -> CreateShareArgs {
    CreateShareArgs {
        target: args.target.clone(),
        instance: args.instance.clone(),
        space: args.space.clone(),
        folder: args.folder.clone(),
        file: args.file.clone(),
        is_by_name: args.is_by_name,
        share_endpoint: args.share_endpoint.clone(),
        settings: args.settings.clone(),
    }
}
async fn resolve_selector(client: &Client, args: &ShareIdArgs) -> Result<ShareTarget, CliError> {
    // [Review Fix #ShareTarget1] Both CLI forms must select the same identity before name resolution or authenticated API calls.
    let mut target = selector_target(args)?;
    if args.is_by_name {
        target.instance =
            super::high_level::resolve_instance_segment_by_name(client, &target.instance).await?;
    }
    Ok(target)
}
fn configured_share_origin(
    global: &GlobalArgs,
    explicit: Option<&str>,
) -> Result<Option<String>, CliError> {
    let config = ConfigFile::load_from(&global.existing_runtime_config_path()?)?;
    let configured = config
        .profiles
        .get(&global.profile)
        .and_then(|profile| profile.adrive.as_ref())
        .and_then(|settings| settings.share_endpoint.as_deref());
    // [Review Fix #ShareEndpoint3] Prefer the canonical variable while retaining old deployment configuration.
    let environment = std::env::var("ADRIVE_SHARE_ENDPOINT")
        .ok()
        .or_else(|| std::env::var("ADRIVE_SHARE_BASE_URL").ok());
    // [Review Fix #ShareOrigin2] Validate after precedence so an unused invalid origin cannot block an explicit safe override.
    if let Some(origin) = explicit.or(configured).or(environment.as_deref()) {
        validate_origin(Some(origin))?;
        return Ok(Some(origin.into()));
    }
    // [Review Fix #ServerShareURL1] Missing page configuration selects the service URL, never an inferred API hostname.
    Ok(None)
}

async fn create_link<'origin>(
    client: &Client,
    args: &CreateShareArgs,
    origin: impl Into<Option<&'origin str>>,
) -> Result<Value, CliError> {
    let origin = origin.into();
    validate_origin(origin)?;
    // [Review Fix #ShareSpace2] ShareAllFiles is the V1 whole-Space contract; never send an empty file path.
    let (target, mut input) = create_input(args)?;
    if args.settings.is_password_stdin {
        input.share_pwd = Some(super::share_password::read_password_stdin().await?);
    }
    let info = client
        .create_share_link(&input)
        .await
        .map_err(mutation_error)?;
    // [Review Fix #ShareSpace3] A contradictory scope cannot be delivered as the requested share or retried as a new creation.
    if info
        .is_share_all_files
        .is_some_and(|is_all| is_all != input.is_share_all_files)
    {
        return Err(unknown_result("create response sharing scope differs from the request; reconcile with share list/get before another create"));
    }
    if info
        .share_id
        .as_deref()
        .is_none_or(|id| id.trim().is_empty())
    {
        return Err(unknown_result(
            "create response omitted ShareID; reconcile with share list before another create",
        ));
    }
    if info
        .share_id
        .as_deref()
        .is_some_and(|id| validate_share_id_component(id).is_err())
    {
        return Err(unknown_result("create response supplied an invalid ShareID; reconcile with share list before another create"));
    }
    // [Review Fix #ShareSubmittedIdentity1] A 2xx mutation with inconsistent metadata cannot establish the submitted outcome.
    delivery_share_info_value(info, &target.instance, None, origin).map_err(|_| {
        unknown_result("create response identity is incomplete or inconsistent; reconcile with share list/get before another create")
    })
}
#[cfg(test)]
async fn get_link(client: &Client, args: &ShareIdArgs, origin: &str) -> Result<Value, CliError> {
    let target = selector_target(args)?;
    get_resolved_link(client, &target, Some(origin)).await
}
async fn get_resolved_link(
    client: &Client,
    target: &ShareTarget,
    origin: Option<&str>,
) -> Result<Value, CliError> {
    let info = client
        .get_share_link(&GetShareLinkInput {
            instance_id: target.instance.clone(),
            share_id: target.share_id.clone(),
        })
        .await
        .map_err(map_ids_error)?;
    delivery_share_info_value(info, &target.instance, Some(&target.share_id), origin)
}
#[cfg(test)]
async fn update_link(
    client: &Client,
    args: &UpdateShareArgs,
    origin: &str,
) -> Result<Value, CliError> {
    let target = selector_target(&args.selector)?;
    update_resolved_link(client, args, &target, Some(origin)).await
}
async fn update_resolved_link(
    client: &Client,
    args: &UpdateShareArgs,
    target: &ShareTarget,
    origin: Option<&str>,
) -> Result<Value, CliError> {
    validate_origin(origin)?;
    validate_settings(&args.settings)?;
    if !has_changes(&args.settings) {
        return Err(validation(
            "update requires at least one changed share setting",
        ));
    }
    let updated_at = update_guard(client, args, target).await?;
    let mutable_fields = settings_input(&args.settings)?;
    let share_pwd = if args.settings.is_password_stdin {
        Some(super::share_password::read_password_stdin().await?)
    } else {
        None
    };
    let input = UpdateShareLinkInput {
        instance_id: target.instance.clone(),
        share_id: target.share_id.clone(),
        updated_at,
        share_pwd,
        share_name: mutable_fields.share_name,
        description: mutable_fields.description,
        permission: mutable_fields.permission,
        expiration_sec: mutable_fields.expiration_sec,
        require_login: mutable_fields.require_login,
        preview_limit: mutable_fields.preview_limit,
        download_limit: mutable_fields.download_limit,
        ..Default::default()
    };
    let info = client
        .update_share_link(&input)
        .await
        .map_err(|error| mutation_error_for_id(error, &target.share_id))?;
    // [Review Fix #ShareSubmittedIdentity1] Preserve the submitted selector when a successful response cannot confirm its identity.
    share_info_value(
        info,
        &target.instance,
        Some(&target.share_id),
        origin,
        false,
    ).map_err(|_| unknown_result(&format!("update response identity is incomplete or inconsistent for ShareID {}; reconcile with share get/list before retrying",target.share_id)))
}
async fn update_guard(
    client: &Client,
    args: &UpdateShareArgs,
    target: &ShareTarget,
) -> Result<i64, CliError> {
    match args.updated_at {
        Some(value) => Ok(value),
        None => {
            let info = client
                .get_share_link(&GetShareLinkInput {
                    instance_id: target.instance.clone(),
                    share_id: target.share_id.clone(),
                })
                .await
                .map_err(map_ids_error)?;
            validate_instance(&info, &target.instance)?;
            validate_share_id(&info, &target.share_id)?;
            info.updated_at.ok_or_else(|| {
                validation("share get omitted UpdatedAt; supply an explicit --updated-at guard")
            })
        }
    }
}

fn search_input(args: &ListShareArgs, instance: String) -> SearchShareLinksInput {
    SearchShareLinksInput {
        instance_id: instance,
        creators: args.creator.clone(),
        status: args.status.map(|value| value.as_str().into()),
        share_name: args.name.clone(),
        created_at_start: args.created_at_start.clone(),
        created_at_end: args.created_at_end.clone(),
        offset: args.offset,
        limit: args.limit,
    }
}
async fn list_links(
    global: &GlobalArgs,
    client: &Client,
    args: &ListShareArgs,
) -> Result<Value, CliError> {
    let selected_instance = list_instance(args)?;
    let instance = if args.is_by_name {
        super::high_level::resolve_instance_segment_by_name(client, &selected_instance).await?
    } else {
        selected_instance
    };
    let origin = configured_share_origin(global, args.share_endpoint.as_deref())?;
    let mut input = search_input(args, instance.clone());
    if input.creators.is_empty() {
        input.creators=vec![client.oauth_user_id().map_err(map_ids_error)?.filter(|id|!id.trim().is_empty()).ok_or_else(||validation("share list requires --creator unless OAuth credentials contain a bound user_id"))?];
    }
    let result = client
        .search_share_links(&input)
        .await
        .map_err(map_ids_error)?;
    // [Review Fix #ShareScope1] Some responses carry their scope at the collection level.
    if result
        .additional_fields
        .get("InstanceID")
        .and_then(Value::as_str)
        .is_some_and(|returned| returned != instance)
    {
        return Err(validation(
            "share response InstanceID does not match the requested instance",
        ));
    }
    let items = result
        .items
        .iter()
        .cloned()
        .map(|info| share_info_value(info, &instance, None, origin.as_deref(), false))
        .collect::<Result<Vec<_>, _>>()?;
    let mut value = serde_json::to_value(result)?;
    // [Review Fix #ShareCollectionSecret1] Redact collection extensions before restoring already sanitized items and their safe page URLs.
    redact_share_payload(&mut value);
    value["Items"] = json!(items);
    Ok(value)
}
fn validate_instance(info: &ShareLinkInfo, instance: &str) -> Result<(), CliError> {
    if info
        .instance_id
        .as_deref()
        .is_some_and(|returned| returned != instance)
    {
        return Err(validation(
            "share response InstanceID does not match the requested instance",
        ));
    }
    Ok(())
}
/// Validate a supplied response ShareID against the known selector without echoing it.
/// Missing optional IDs remain compatible; explicit different or empty IDs fail.
pub(crate) fn validate_share_id(info: &ShareLinkInfo, expected_id: &str) -> Result<(), CliError> {
    // [Review Fix #ShareIdentity1] A response from another share must never supply output data or an optimistic version guard.
    if info
        .share_id
        .as_deref()
        .is_some_and(|returned_id| returned_id != expected_id)
    {
        return Err(validation(
            "share response ShareID does not match the requested share",
        ));
    }
    Ok(())
}

fn delivery_share_info_value(
    info: ShareLinkInfo,
    instance: &str,
    known_id: Option<&str>,
    origin: Option<&str>,
) -> Result<Value, CliError> {
    // [Review Fix #ShareDelivery1] Only approved create/get business results may return the service's delivery password; diagnostics and other output remain redacted.
    let delivery_password = info.share_pwd.clone();
    let mut value = share_info_value(info, instance, known_id, origin, true)?;
    if let Some(password) = delivery_password {
        value["SharePwd"] = json!(password);
    }
    Ok(value)
}

fn share_info_value(
    mut info: ShareLinkInfo,
    instance: &str,
    known_id: Option<&str>,
    origin: Option<&str>,
    is_delivery: bool,
) -> Result<Value, CliError> {
    validate_instance(&info, instance)?;
    if let Some(expected_id) = known_id {
        validate_share_id(&info, expected_id)?;
    }
    if info.share_id.is_none() {
        info.share_id = known_id.map(str::to_owned);
    }
    // [Review Fix #ShareId1] Invalid or missing collection IDs remain metadata and never create a page URL.
    let share_url = resolved_share_url(&info, origin, is_delivery);
    let mut value = serde_json::to_value(info)?;
    redact_share_payload(&mut value);
    if let Some(url) = share_url {
        value["ShareUrl"] = json!(url);
    } else if origin.is_none() {
        // [Review Fix #ServerShareURL2/#ServerShareURL4] URL absence cannot undo create; list metadata may also lack an ID, so retain metadata without claiming validity.
        value["ShareUrlWarning"] = json!("Service response omitted a usable ShareURL. Keep the returned ShareID and configure --share-endpoint or use share get; do not repeat a successful create.");
    }
    Ok(value)
}

fn resolved_share_url(
    info: &ShareLinkInfo,
    origin: Option<&str>,
    is_delivery: bool,
) -> Option<String> {
    let share_id = info.share_id.as_deref()?;
    if let Some(origin) = origin {
        return build_share_url(origin, share_id).ok();
    }
    let returned = info.share_url.as_deref()?;
    let parsed = parse_share_url(returned).ok()?;
    if parsed.share_id != share_id {
        return None;
    }
    // [Review Fix #ServerShareURL3] Only create/get may deliver a password-bearing URL; list/update retain their existing redaction boundary.
    Some(if is_delivery {
        returned.to_owned()
    } else {
        parsed.safe_url
    })
}
/// Remove secrets and signed receiver URLs from service-provided nested payloads.
pub(crate) fn redact_share_payload(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| {
                !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "sharepwd"
                        | "share_pwd"
                        | "share-pwd"
                        | "sharepassword"
                        | "x-share-token"
                        | "shareurl"
                        | "password"
                        | "access_token"
                        | "accesstoken"
                        | "downloadurl"
                        | "download_url"
                )
            });
            for child in object.values_mut() {
                redact_share_payload(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_share_payload),
        _ => {}
    }
}
fn mutation_error(error: Error) -> CliError {
    match &error {
        Error::Server(server)
            if server
                .status_code
                .is_some_and(|status| status < 500 && status != 408) =>
        {
            map_ids_error(error)
        }
        Error::Client(_) | Error::Cli(_) | Error::UnifiedCredential(_) => map_ids_error(error),
        _ => unknown_result(
            "share mutation outcome is unknown; reconcile with share get/list before retrying",
        ),
    }
}
fn mutation_error_for_id(error: Error, share_id: &str) -> CliError {
    let result = mutation_error(error);
    if result.to_string().contains("share_result_unknown") {
        return unknown_result(&format!("share mutation outcome is unknown for ShareID {share_id}; reconcile with share get/list before retrying"));
    }
    result
}
fn unknown_result(message: &str) -> CliError {
    CliError::Unknown(format!("[share_result_unknown] {message}"))
}
async fn confirm_cancel(global: &GlobalArgs) -> Result<(), CliError> {
    if global.yes {
        return Ok(());
    }
    if !super::share_password::can_prompt(global) {
        return Err(validation(
            "share cancel requires global --yes in noninteractive, JSON or quiet execution",
        ));
    }
    eprint!("Cancel this share link? [y/N] ");
    std::io::stderr().flush()?;
    // [Review Fix #ShareInterrupt1] Blocking stdin would prevent Ctrl-C from being observed during confirmation.
    let answer = super::share_password::read_confirmation_line().await?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Err(validation("share cancellation was not confirmed"));
    }
    Ok(())
}
fn share_plan(action: &ShareAction) -> Result<Value, CliError> {
    let selection = match action {
        ShareAction::Create(args) => {
            let (target, input) = create_input(args)?;
            json!({"InstanceID":target.instance,"SpaceID":target.space,"FilePathList":input.file_path_list,"ShareAllFiles":input.is_share_all_files,"Settings":planned_settings(&args.settings,true)?,"PasswordSource":if args.settings.is_password_stdin {Some("stdin")}else{None}})
        }
        ShareAction::Get(args) => {
            let target = selector_target(args)?;
            json!({"InstanceID":target.instance,"ShareID":target.share_id})
        }
        ShareAction::Cancel(args) => {
            let target = selector_target(&cancel_selector(args))?;
            json!({"InstanceID":target.instance,"ShareID":target.share_id})
        }
        ShareAction::Update(args) => {
            let target = selector_target(&args.selector)?;
            json!({"InstanceID":target.instance,"ShareID":target.share_id,"UpdatedAt":args.updated_at,"Settings":planned_settings(&args.settings,false)?,"PasswordSource":if args.settings.is_password_stdin {Some("stdin")}else{None}})
        }
        ShareAction::List(args) => {
            json!({"InstanceID":list_instance(args)?,"Creators":args.creator})
        }
        ShareAction::Inspect(args) => {
            json!({"ShareUrl":parse_share_url(&args.share_url).map_err(map_ids_error)?.safe_url})
        }
        ShareAction::Files(args) => {
            // [Review Fix #ShareFilesPlan1] Offline plans must show and validate the same cap/cursor as execution.
            json!({"ShareUrl":parse_share_url(&args.share_url).map_err(map_ids_error)?.safe_url,"Path":args.path,"MaxKeys":args.max_keys,"Marker":args.marker})
        }
        ShareAction::View(args) => {
            json!({"ShareUrl":parse_share_url(&args.share_url).map_err(map_ids_error)?.safe_url,"Path":args.path,"Raw":args.is_raw,"NetworkRequests":0,"ConsumesPreviewLimit":true})
        }
        ShareAction::Download(_) => {
            return Err(validation(
                "download planning requires the read-only metadata planner",
            ))
        }
    };
    Ok(
        json!({"dry_run":true,"action":action.name(),"selection":selection,"execution_status":"planned_not_executed"}),
    )
}

fn planned_settings(settings: &ShareSettings, is_create: bool) -> Result<Value, CliError> {
    let mut input = settings_input(settings)?;
    if is_create {
        input.expiration_sec = Some(input.expiration_sec.unwrap_or(0));
        input.permission = Some(
            input
                .permission
                .unwrap_or_else(|| DEFAULT_CREATE_SHARE_PERMISSION.into()),
        );
    }
    let mut value = serde_json::to_value(input)?;
    if let Some(object) = value.as_object_mut() {
        for key in ["InstanceID", "SpaceID", "FilePathList"] {
            object.remove(key);
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::client::ClientOptions;
    use crate::domain::share_access::fixture::{reply, Fixture};
    use clap::Parser;
    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        action: ShareAction,
    }
    fn action(arguments: &[&str]) -> ShareAction {
        TestCli::parse_from(std::iter::once("test").chain(arguments.iter().copied())).action
    }

    #[test]
    fn whole_space_create_plans_select_all_files_without_an_empty_file_path() {
        for arguments in [
            vec!["create", "adrive://inst/space"],
            vec!["create", "adrive://inst/space/"],
            vec!["create", "--instance", "inst", "--space", "space"],
            vec![
                "create",
                "--instance",
                "inst",
                "--space",
                "space",
                "--folder",
                "/",
            ],
        ] {
            let action = action(&arguments);
            validate_action(&action).unwrap();
            let plan = share_plan(&action).unwrap();
            assert_eq!(plan["selection"]["ShareAllFiles"], true);
            assert_eq!(plan["selection"]["FilePathList"], json!([]));
            assert_eq!(plan["selection"]["InstanceID"], "inst");
            assert_eq!(plan["selection"]["SpaceID"], "space");
        }
    }

    #[tokio::test]
    async fn whole_space_create_sends_v1_scope_and_uses_the_delivered_server_url() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"ShareID":"space-share","InstanceID":"inst","SpaceID":"space","ShareAllFiles":true,"ShareURL":"https://pages.test/share/space-share"}"#,
        )]);
        let ShareAction::Create(args) = action(&["create", "adrive://inst/space"]) else {
            panic!()
        };
        let result = create_link(&client(fixture.endpoint.clone()), &args, None)
            .await
            .unwrap();
        assert_eq!(result["ShareUrl"], "https://pages.test/share/space-share");
        assert_eq!(result["ShareAllFiles"], true);
        let requests = fixture.finish();
        assert_eq!(requests.len(), 1);
        let body: Value =
            serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["ShareAllFiles"], true);
        assert_eq!(body["FilePathList"], json!([]));
        assert_eq!(body["SpaceID"], "space");
        assert_eq!(body["Permission"], "view");
        assert_eq!(body["ExpirationSec"], 0);
    }

    #[test]
    fn explicitly_empty_selectors_do_not_expand_to_whole_space() {
        for selector in ["--file", "--folder"] {
            let ShareAction::Create(args) = action(&[
                "create",
                "--instance",
                "inst",
                "--space",
                "space",
                selector,
                "",
            ]) else {
                panic!()
            };
            assert!(create_target(&args).is_err());
        }
        assert!(validate_action(&action(&["create", "adrive://inst"])).is_err());
        assert!(validate_action(&action(&["create", "adrive://inst/space/   "])).is_err());
    }

    #[test]
    fn ambiguous_slash_only_targets_cannot_expand_sharing_to_the_whole_space() {
        for arguments in [
            vec!["create", "adrive://inst/space//"],
            vec!["create", "adrive://inst/space////"],
            vec![
                "create",
                "--instance",
                "inst",
                "--space",
                "space",
                "--folder",
                "///",
            ],
            vec![
                "create",
                "--instance",
                "inst",
                "--space",
                "space",
                "--file",
                "/",
            ],
            vec![
                "create",
                "--instance",
                "inst",
                "--space",
                "space",
                "--file",
                "///",
            ],
        ] {
            assert!(
                validate_action(&action(&arguments)).is_err(),
                "accepted {arguments:?}"
            );
        }
    }

    #[tokio::test]
    async fn contradictory_created_scope_is_unknown_without_replaying_create() {
        for (target, flag) in [
            ("adrive://inst/space", false),
            ("adrive://inst/space/file.txt", true),
        ] {
            let response = json!({"ShareID":"s","InstanceID":"inst","ShareAllFiles":flag,"ShareURL":"https://pages.test/share/s"});
            let fixture = Fixture::new(vec![reply(200, &response.to_string())]);
            let ShareAction::Create(args) = action(&["create", target]) else {
                panic!()
            };
            let error = create_link(&client(fixture.endpoint.clone()), &args, None)
                .await
                .unwrap_err();
            assert_eq!(error.agent_semantics().code, "share_result_unknown");
            assert_eq!(fixture.finish().len(), 1);
        }
    }

    #[test]
    fn explicit_receiver_space_root_is_valid_only_for_listing_and_download() {
        for arguments in [
            vec!["files", "https://page.test/share/s", "--path", "/"],
            vec![
                "download",
                "https://page.test/share/s",
                "./out",
                "--path",
                "/",
                "--recursive",
            ],
        ] {
            validate_action(&action(&arguments)).unwrap();
        }
        assert!(validate_action(&action(&[
            "view",
            "https://page.test/share/s",
            "--path",
            "/"
        ]))
        .is_err());
    }

    #[test]
    fn service_share_urls_preserve_delivery_queries_and_redact_metadata_queries() {
        let returned = "http://inst.adriveapp.volces.com:8080/share/s?pwd=p%2Bq%20%26";
        for (is_delivery, expected) in [
            (true, returned),
            (false, returned.split('?').next().unwrap()),
        ] {
            let info = serde_json::from_value(json!({"ShareID":"s", "ShareURL":returned})).unwrap();
            let value = share_info_value(info, "inst", None, None, is_delivery).unwrap();
            assert_eq!(value["ShareUrl"], expected);
            assert!(value.get("ShareURL").is_none());
            assert!(value.get("ShareUrlWarning").is_none());
        }
    }

    #[test]
    fn missing_or_unusable_service_share_urls_preserve_id_with_warning() {
        for returned in [
            None,
            Some(""),
            Some("https://pages.test/share/other"),
            Some("https://user:secret@pages.test/share/s"),
            Some("javascript:secret"),
        ] {
            let info = serde_json::from_value(json!({"ShareID":"s", "ShareURL":returned})).unwrap();
            let value = share_info_value(info, "inst", None, None, true).unwrap();
            assert_eq!(value["ShareID"], "s");
            assert!(value.get("ShareUrl").is_none());
            assert!(value["ShareUrlWarning"]
                .as_str()
                .unwrap()
                .contains("--share-endpoint"));
            assert!(!value.to_string().contains("secret"));
        }
    }

    #[test]
    fn configured_page_origin_overrides_service_url_without_delivery_query() {
        let info = serde_json::from_value(
            json!({"ShareID":"s", "ShareURL":"https://service.test/share/s?pwd=service-secret"}),
        )
        .unwrap();
        let value =
            share_info_value(info, "inst", None, Some("http://custom.test:8080"), true).unwrap();
        assert_eq!(value["ShareUrl"], "http://custom.test:8080/share/s");
        assert!(!value.to_string().contains("service-secret"));
    }

    #[test]
    fn files_plan_exposes_max_keys_and_marker_with_local_validation() {
        let selected = action(&[
            "files",
            "https://pages.test/share/s",
            "--max-keys",
            "3",
            "--marker",
            "next",
        ]);
        validate_action(&selected).unwrap();
        let plan = share_plan(&selected).unwrap();
        assert_eq!(plan["selection"]["MaxKeys"], 3);
        assert_eq!(plan["selection"]["Marker"], "next");
        let invalid = action(&[
            "files",
            "https://pages.test/share/s",
            "--marker",
            "bad\nmarker",
        ]);
        assert!(validate_action(&invalid).is_err());
    }

    #[test]
    fn share_management_uri_plans_match_legacy_selectors() {
        for (verb, options) in [
            ("get", vec![]),
            ("update", vec!["--name", "New"]),
            ("cancel", vec![]),
        ] {
            let mut uri = vec!["test", verb, "adrive://vtagt3g9n/70PIsJo5AOz"];
            uri.extend(options.iter().copied());
            let selected = TestCli::try_parse_from(uri).unwrap().action;
            validate_action(&selected).unwrap();
            let mut legacy = vec![verb, "70PIsJo5AOz", "--instance", "vtagt3g9n"];
            legacy.extend(options.iter().copied());
            assert_eq!(
                share_plan(&selected).unwrap(),
                share_plan(&action(&legacy)).unwrap()
            );
        }
    }

    #[test]
    fn share_management_uri_rejects_conflicting_instance_before_io() {
        for verb in ["get", "cancel"] {
            let selected = action(&[verb, "adrive://inst/s", "--instance", "other"]);
            assert!(validate_action(&selected).is_err());
        }
    }

    #[test]
    fn share_list_instance_uri_matches_legacy_plan() {
        let uri = TestCli::try_parse_from(["test", "list", "adrive://inst", "--creator", "user"])
            .unwrap()
            .action;
        validate_action(&uri).unwrap();
        let legacy = action(&["list", "--instance", "inst", "--creator", "user"]);
        assert_eq!(share_plan(&uri).unwrap(), share_plan(&legacy).unwrap());
    }

    #[test]
    fn share_list_instance_uri_rejects_conflict_before_io() {
        let uri = TestCli::try_parse_from(["test", "list", "adrive://inst", "--instance", "other"])
            .unwrap()
            .action;
        assert!(validate_action(&uri).is_err());
    }

    #[tokio::test]
    async fn share_list_instance_uri_preserves_search_filters_and_safe_output() {
        let response = r#"{"InstanceID":"inst","Items":[{"ShareID":"s","InstanceID":"inst","SharePwd":"hidden"}],"Total":1,"Offset":3,"Limit":7}"#;
        let fixture = Fixture::new(vec![reply(200, response), reply(200, response)]);
        // [Review Fix #ShareList4] Explicit page origins still load config; isolate this fixture from the developer profile.
        let (global, directory) = runtime_config(&fixture.endpoint);
        let mut results = Vec::new();
        for selector in [vec!["adrive://inst/"], vec!["--instance", "inst"]] {
            let ShareAction::List(args) = filtered_list_action(&selector) else {
                panic!()
            };
            results.push(
                list_links(&global, &client(fixture.endpoint.clone()), &args)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(
            results[0]["Items"][0]["ShareUrl"],
            "http://pages.test/share/s"
        );
        assert!(!results[0].to_string().contains("hidden"));
        assert_eq!(results[0]["Offset"], 3);
        let requests = fixture.finish();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert!(request.starts_with("GET /v1/share_link/search?"));
            for filter in [
                "creators=u",
                "status=active",
                "share_name=Docs",
                "offset=3",
                "limit=7",
            ] {
                assert!(request.contains(filter), "missing {filter}");
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn filtered_list_action(selector: &[&str]) -> ShareAction {
        let mut arguments = vec!["list"];
        arguments.extend_from_slice(selector);
        arguments.extend([
            "--creator",
            "u",
            "--status",
            "active",
            "--name",
            "Docs",
            "--offset",
            "3",
            "--limit",
            "7",
            "--share-endpoint",
            "http://pages.test",
        ]);
        action(&arguments)
    }

    #[tokio::test]
    async fn share_list_instance_uri_by_name_resolves_the_instance() {
        let fixture = Fixture::new(vec![
            reply(
                200,
                r#"{"Instance":{"InstanceID":"inst","Name":"My Instance"}}"#,
            ),
            reply(200, r#"{"InstanceID":"inst","Items":[],"Total":0}"#),
        ]);
        let (global, directory) = runtime_config(&fixture.endpoint);
        let result = manage(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(crate::domain::auth::AuthMode::Aksk),
            },
            &action(&[
                "list",
                "adrive://My%20Instance",
                "--by-name",
                "--creator",
                "u",
                "--share-endpoint",
                "http://pages.test",
            ]),
        )
        .await
        .unwrap();
        assert_eq!(result["Items"], json!([]));
        let requests = fixture.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("name=My+Instance"));
        assert!(requests[1].starts_with("GET /v1/share_link/search?"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn share_management_uri_keeps_decoded_share_id_opaque_in_api_requests() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"ShareID":"adrive://nested/s","InstanceID":"inst"}"#,
        )]);
        let (global, directory) = runtime_config(&fixture.endpoint);
        let selected = action(&[
            "get",
            "adrive://inst/adrive%3A%2F%2Fnested%2Fs",
            "--share-endpoint",
            "http://pages.test",
        ]);
        let result = manage(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(crate::domain::auth::AuthMode::Aksk),
            },
            &selected,
        )
        .await
        .unwrap();
        assert_eq!(result["ShareID"], "adrive://nested/s");
        let requests = fixture.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("share_id=adrive%3A%2F%2Fnested%2Fs"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn share_management_uri_by_name_resolves_only_the_instance() {
        let fixture = Fixture::new(vec![
            reply(
                200,
                r#"{"Instance":{"InstanceID":"inst","Name":"My Instance"}}"#,
            ),
            reply(200, r#"{"ShareID":"s","InstanceID":"inst"}"#),
        ]);
        let (global, directory) = runtime_config(&fixture.endpoint);
        let selected = action(&[
            "get",
            "adrive://My%20Instance/s",
            "--by-name",
            "--share-endpoint",
            "http://pages.test",
        ]);
        let result = manage(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(crate::domain::auth::AuthMode::Aksk),
            },
            &selected,
        )
        .await
        .unwrap();
        assert_eq!(result["ShareID"], "s");
        let requests = fixture.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("name=My+Instance"));
        assert!(requests[1].contains("share_id=s"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn share_management_uri_update_and_cancel_use_decoded_api_identity() {
        // [Review Fix #ShareTarget2] Exercise an opaque URI-like ID through the guard and mutations so normalized identities cannot be reparsed.
        let share_id = "adrive://nested/s";
        let observed = json!({"ShareID":share_id,"InstanceID":"inst","UpdatedAt":7}).to_string();
        let updated = json!({"ShareID":share_id,"InstanceID":"inst"}).to_string();
        for (verb, replies, options) in [
            (
                "update",
                vec![reply(200, &observed), reply(200, &updated)],
                vec!["--name", "New", "--share-endpoint", "http://pages.test"],
            ),
            ("cancel", vec![reply(204, "")], vec![]),
        ] {
            let fixture = Fixture::new(replies);
            let (mut global, directory) = runtime_config(&fixture.endpoint);
            global.yes = true;
            let mut arguments = vec![verb, "adrive://inst/adrive%3A%2F%2Fnested%2Fs"];
            arguments.extend(options);
            let result = manage(
                &global,
                &ADriveAuthArgs {
                    auth_mode: Some(crate::domain::auth::AuthMode::Aksk),
                },
                &action(&arguments),
            )
            .await
            .unwrap();
            assert_eq!(result["ShareID"], share_id);
            let requests = fixture.finish();
            let submission = requests.last().unwrap();
            assert!(submission.contains("\"ShareID\":\"adrive://nested/s\""));
            assert!(!submission.contains("adrive%3A"));
            if verb == "update" {
                assert!(requests[0].contains("share_id=adrive%3A%2F%2Fnested%2Fs"));
            }
            std::fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn share_management_uri_interruption_reports_the_decoded_share_id() {
        for verb in ["update", "cancel"] {
            let mut arguments = vec![verb, "adrive://inst/adrive%3A%2F%2Fnested%2Fs"];
            if verb == "update" {
                arguments.extend(["--name", "New"]);
            }
            let error = interrupted_action_error(&action(&arguments), true);
            assert!(error.to_string().contains("ShareID adrive://nested/s"));
            assert!(!error.to_string().contains("adrive%3A"));
            assert_eq!(error.agent_semantics().code, "share_result_unknown");
        }
    }
    fn client(endpoint: String) -> Client {
        Client::new(
            "fixture-ak".into(),
            "fixture-sk".into(),
            None,
            Some(endpoint),
            Some("cn-beijing".into()),
            ClientOptions {
                max_retry_count: Some(0),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn cancel_ignores_unused_invalid_page_origin_and_instance_dns_label() {
        let fixture = Fixture::new(vec![reply(204, "")]);
        let (mut global, directory) = runtime_config(&fixture.endpoint);
        global.yes = true;
        let args = action(&["cancel", "s", "--instance", "instance_with_underscore"]);
        let result = manage(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(crate::domain::auth::AuthMode::Aksk),
            },
            &args,
        )
        .await
        .unwrap();
        assert_eq!(result["Cancelled"], true);
        assert_eq!(fixture.finish().len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn successful_update_with_wrong_identity_reports_unknown_outcome_without_replay() {
        for body in [
            r#"{"ShareID":"other","InstanceID":"wrong"}"#,
            r#"{"ShareID":"other","InstanceID":"inst"}"#,
        ] {
            let fixture = Fixture::new(vec![reply(200, body)]);
            let ShareAction::Update(args) = action(&[
                "update",
                "requested",
                "--instance",
                "inst",
                "--updated-at",
                "7",
                "--name",
                "New",
            ]) else {
                panic!()
            };
            let error = update_link(
                &client(fixture.endpoint.clone()),
                &args,
                "https://pages.test",
            )
            .await
            .unwrap_err();
            assert_eq!(error.agent_semantics().code, "share_result_unknown");
            assert_eq!(error.exit_code().as_i32(), 1);
            assert!(error.to_string().contains("requested"));
            assert!(error.to_string().contains("reconcile"));
            assert_eq!(fixture.finish().len(), 1);
        }
    }

    #[tokio::test]
    async fn successful_create_with_inconsistent_or_unusable_identity_reports_unknown_outcome() {
        for body in [
            r#"{"ShareID":"created","InstanceID":"wrong"}"#,
            r#"{"ShareID":"bad\u0000id","InstanceID":"inst"}"#,
        ] {
            let fixture = Fixture::new(vec![reply(200, body)]);
            let ShareAction::Create(args) = action(&["create", "adrive://inst/space/a"]) else {
                panic!()
            };
            let error = create_link(
                &client(fixture.endpoint.clone()),
                &args,
                "https://pages.test",
            )
            .await
            .unwrap_err();
            assert_eq!(error.agent_semantics().code, "share_result_unknown");
            assert_eq!(error.exit_code().as_i32(), 1);
            assert!(error.to_string().contains("reconcile"));
            assert_eq!(fixture.finish().len(), 1);
        }
    }

    #[tokio::test]
    async fn create_share_sends_defaults_and_constructs_one_page_url() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"ShareID":"s","InstanceID":"inst","FilePathList":["docs/"]}"#,
        )]);
        let ShareAction::Create(args) = action(&["create", "adrive://inst/space/docs/"]) else {
            panic!()
        };
        let result = create_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap();
        assert_eq!(result["ShareUrl"], "https://pages.test/share/s");
        let requests = fixture.finish();
        assert!(requests[0].contains("\"ExpirationSec\":0"));
        // [Review Fix #ShareDefault2] Existing create request coverage must assert the PRD preview-only default too.
        assert!(requests[0].contains("\"Permission\":\"view\""));
        assert!(requests[0].contains("\"FilePathList\":[\"docs/\"]"));
    }
    #[tokio::test]
    async fn update_share_fetches_guard_and_retains_explicit_false_zero_empty() {
        let fixture = Fixture::new(vec![
            reply(200, r#"{"ShareID":"s","InstanceID":"inst","UpdatedAt":42}"#),
            reply(200, r#"{"ShareID":"s"}"#),
        ]);
        let ShareAction::Update(args) = action(&[
            "update",
            "s",
            "--instance",
            "inst",
            "--name",
            "",
            "--require-login",
            "false",
            "--expires-in",
            "0",
        ]) else {
            panic!()
        };
        update_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap();
        let requests = fixture.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("\"UpdatedAt\":42"));
        assert!(requests[1].contains("\"RequireLogin\":false"));
        assert!(requests[1].contains("\"ShareName\":\"\""));
        assert!(requests[1].contains("\"ExpirationSec\":0"));
    }
    #[tokio::test]
    async fn explicit_update_guard_skips_get_and_preserves_known_id() {
        let fixture = Fixture::new(vec![reply(200, "{}")]);
        let ShareAction::Update(args) = action(&[
            "update",
            "known",
            "--instance",
            "inst",
            "--updated-at",
            "7",
            "--download-limit",
            "0",
        ]) else {
            panic!()
        };
        let value = update_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap();
        assert_eq!(value["ShareID"], "known");
        let requests = fixture.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("PUT /v1/share_link/update"));
    }
    #[tokio::test]
    async fn known_mutation_4xx_keeps_common_error_semantics() {
        let fixture = Fixture::new(vec![reply(409, r#"{"Code":"Conflict"}"#)]);
        let ShareAction::Update(args) = action(&[
            "update",
            "s",
            "--instance",
            "inst",
            "--updated-at",
            "7",
            "--name",
            "New",
        ]) else {
            panic!()
        };
        let error = update_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap_err();
        assert!(matches!(error, CliError::Conflict(_)));
        assert_eq!(fixture.finish().len(), 1);
    }
    #[tokio::test]
    async fn get_rejects_response_instance_mismatch() {
        let fixture = Fixture::new(vec![reply(200, r#"{"ShareID":"s","InstanceID":"other"}"#)]);
        let ShareAction::Get(args) = action(&["get", "s", "--instance", "inst"]) else {
            panic!()
        };
        let error = get_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
        fixture.finish();
    }
    #[tokio::test]
    async fn list_preserves_wire_pagination_and_does_not_invent_missing_ids() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"Items":[{"ShareID":"s","InstanceID":"inst"},{"ShareName":"no-id","ShareID":""}],"Total":4,"Offset":1,"Limit":2}"#,
        )]);
        let path = std::env::temp_dir().join(format!("share-list-{}.toml", ulid::Ulid::new()));
        std::fs::write(&path, "").unwrap();
        let global = GlobalArgs {
            config_path: Some(path.clone()),
            ..Default::default()
        };
        let ShareAction::List(args) = action(&[
            "list",
            "--instance",
            "inst",
            "--creator",
            "u",
            "--offset",
            "1",
            "--limit",
            "2",
        ]) else {
            panic!()
        };
        let value = list_links(&global, &client(fixture.endpoint.clone()), &args)
            .await
            .unwrap();
        assert_eq!(value["Total"], 4);
        assert_eq!(value["Offset"], 1);
        assert_eq!(value["Limit"], 2);
        assert!(value["Items"][1].get("ShareUrl").is_none());
        fixture.finish();
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn list_redacts_collection_secrets_and_retains_safe_items_and_metadata() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{
                "Items":[{"ShareID":"s","InstanceID":"inst","SharePwd":"item-secret"}],
                "Total":4,"Offset":1,"Limit":2,"SharePwd":"collection-secret",
                "ShareUrl":"https://unsafe.test/share/s?pwd=query-secret",
                "ExtraPayload":{"access_token":"token-secret","Label":"future metadata",
                    "Nested":[{"PASSWORD":"nested-secret","DownloadUrl":"signed-secret","Value":7}]},
                "FutureFlag":true
            }"#,
        )]);
        let path =
            std::env::temp_dir().join(format!("share-list-redaction-{}.toml", ulid::Ulid::new()));
        std::fs::write(&path, "").unwrap();
        let global = GlobalArgs {
            config_path: Some(path.clone()),
            ..Default::default()
        };
        let ShareAction::List(args) = action(&[
            "list",
            "--instance",
            "inst",
            "--creator",
            "u",
            "--share-endpoint",
            "https://pages.test",
        ]) else {
            panic!()
        };
        let value = list_links(&global, &client(fixture.endpoint.clone()), &args)
            .await
            .unwrap();
        for secret in [
            "item-secret",
            "collection-secret",
            "query-secret",
            "token-secret",
            "nested-secret",
            "signed-secret",
        ] {
            assert!(!value.to_string().contains(secret), "leaked {secret}");
        }
        assert!(value.get("SharePwd").is_none());
        assert!(value.get("ShareUrl").is_none());
        assert_eq!(value["Items"][0]["ShareUrl"], "https://pages.test/share/s");
        assert_eq!(value["ExtraPayload"]["Label"], "future metadata");
        assert_eq!(value["ExtraPayload"]["Nested"][0]["Value"], 7);
        assert_eq!(value["FutureFlag"], true);
        assert_eq!(value["Total"], 4);
        assert_eq!(value["Offset"], 1);
        assert_eq!(value["Limit"], 2);
        assert_eq!(fixture.finish().len(), 1);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn share_plan_records_effective_defaults_without_reading_password_stdin() {
        let ShareAction::Create(args) =
            action(&["create", "adrive://inst/space/a", "--password-stdin"])
        else {
            panic!()
        };
        let plan = share_plan(&ShareAction::Create(args)).unwrap();
        assert_eq!(plan["selection"]["Settings"]["Permission"], "view");
        assert_eq!(plan["selection"]["Settings"]["ExpirationSec"], 0);
        assert_eq!(plan["selection"]["PasswordSource"], "stdin");
        assert!(!plan.to_string().contains("SharePwd"));
    }
    #[tokio::test]
    async fn create_permission_matches_prd_default_and_explicit_download() {
        let response = r#"{"ShareID":"s","InstanceID":"inst"}"#;
        let fixture = Fixture::new(vec![reply(200, response), reply(200, response)]);
        let selected_client = client(fixture.endpoint.clone());
        for options in [vec![], vec!["--permission", "download"]] {
            let mut arguments = vec!["create", "adrive://inst/space/docs/"];
            arguments.extend(options);
            let ShareAction::Create(args) = action(&arguments) else {
                panic!()
            };
            create_link(&selected_client, &args, "http://pages.test")
                .await
                .unwrap();
        }
        let requests = fixture.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("\"Permission\":\"view\""));
        assert!(requests[1].contains("\"Permission\":\"download\""));
    }
    #[tokio::test]
    async fn list_rejects_top_level_instance_mismatch() {
        let fixture = Fixture::new(vec![reply(200, r#"{"Items":[],"InstanceID":"other"}"#)]);
        let path = std::env::temp_dir().join(format!("share-list-{}.toml", ulid::Ulid::new()));
        std::fs::write(&path, "").unwrap();
        let global = GlobalArgs {
            config_path: Some(path.clone()),
            ..Default::default()
        };
        let ShareAction::List(args) = action(&["list", "--instance", "inst", "--creator", "u"])
        else {
            panic!()
        };
        assert!(
            list_links(&global, &client(fixture.endpoint.clone()), &args)
                .await
                .is_err()
        );
        fixture.finish();
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn explicit_empty_file_selector_is_rejected() {
        let ShareAction::Create(args) = action(&[
            "create",
            "--instance",
            "inst",
            "--space",
            "space",
            "--folder",
            "docs",
            "--file",
            "",
        ]) else {
            panic!()
        };
        assert!(create_target(&args).is_err());
    }
    #[tokio::test]
    async fn get_rejects_mismatched_share_id() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"ShareID":"other","InstanceID":"inst"}"#,
        )]);
        let ShareAction::Get(args) = action(&["get", "requested", "--instance", "inst"]) else {
            panic!()
        };
        let result = get_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await;
        assert!(result.is_err());
        assert_eq!(fixture.finish().len(), 1);
    }
    #[tokio::test]
    async fn implicit_update_rejects_mismatched_guard_share_id_before_put() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"ShareID":"other","InstanceID":"inst","UpdatedAt":77}"#,
        )]);
        let ShareAction::Update(args) =
            action(&["update", "requested", "--instance", "inst", "--name", "New"])
        else {
            panic!()
        };
        let is_submitted = Arc::new(AtomicBool::new(false));
        let observed_client =
            client(fixture.endpoint.clone()).with_share_submission_observer(is_submitted.clone());
        let result = update_link(&observed_client, &args, "https://pages.test").await;
        let error = result.unwrap_err();
        assert_eq!(error.agent_semantics().code, "InvalidParam");
        assert_eq!(error.exit_code().as_i32(), 6);
        assert!(!is_submitted.load(Ordering::SeqCst));
        let requests = fixture.finish();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].starts_with("PUT"));
    }
    #[tokio::test]
    async fn explicit_update_rejects_mismatched_response_share_id() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"ShareID":"other","InstanceID":"inst"}"#,
        )]);
        let ShareAction::Update(args) = action(&[
            "update",
            "requested",
            "--instance",
            "inst",
            "--updated-at",
            "7",
            "--name",
            "New",
        ]) else {
            panic!()
        };
        assert!(update_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test"
        )
        .await
        .is_err());
        assert_eq!(fixture.finish().len(), 1);
    }
    #[tokio::test]
    async fn create_and_get_preserve_delivery_password_without_query_passwords() {
        let fixture = Fixture::new(vec![
            reply(
                200,
                r#"{"ShareID":"s","InstanceID":"inst","SharePwd":"delivery-secret"}"#,
            ),
            reply(
                200,
                r#"{"ShareID":"s","InstanceID":"inst","SharePwd":"delivery-secret"}"#,
            ),
        ]);
        let client = client(fixture.endpoint.clone());
        let ShareAction::Create(create) = action(&["create", "adrive://inst/space/a"]) else {
            panic!()
        };
        let ShareAction::Get(get) = action(&["get", "s", "--instance", "inst"]) else {
            panic!()
        };
        for result in [
            create_link(&client, &create, "https://pages.test")
                .await
                .unwrap(),
            get_link(&client, &get, "https://pages.test").await.unwrap(),
        ] {
            assert_eq!(result["SharePwd"], "delivery-secret");
            assert_eq!(result["ShareUrl"], "https://pages.test/share/s");
        }
        fixture.finish();
    }
    #[tokio::test]
    async fn update_keeps_delivery_password_redacted() {
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"ShareID":"s","SharePwd":"must-hide"}"#,
        )]);
        let ShareAction::Update(args) = action(&[
            "update",
            "s",
            "--instance",
            "inst",
            "--updated-at",
            "7",
            "--name",
            "New",
        ]) else {
            panic!()
        };
        let result = update_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap();
        assert!(result.get("SharePwd").is_none());
        fixture.finish();
    }
    fn runtime_config(endpoint: &str) -> (GlobalArgs, std::path::PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("share-origin-review-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        std::fs::write(&config_path,format!("[default.adrive]\nregion='cn-beijing'\nendpoint='{endpoint}'\naccess_key_id='fixture-ak'\nsecret_access_key='fixture-sk'\nshare_endpoint='https://user:must-not-echo@bad.test'\n")).unwrap();
        (
            GlobalArgs {
                config_path: Some(config_path),
                ..Default::default()
            },
            directory,
        )
    }

    #[test]
    fn unconfigured_share_page_uses_server_url_without_api_derivation() {
        // [Review Fix #ShareHttpOrigin2] Isolate inherited page overrides in a child process without mutating other tests' environment.
        if std::env::var_os("ADRIVE_SHARE_ENDPOINT").is_some()
            || std::env::var_os("ADRIVE_SHARE_BASE_URL").is_some()
        {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "handler::share::tests::unconfigured_share_page_uses_server_url_without_api_derivation",
                ])
                .env_remove("ADRIVE_SHARE_ENDPOINT")
                .env_remove("ADRIVE_SHARE_BASE_URL")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated default-origin regression failed"
            );
            return;
        }
        let (global, directory) = runtime_config("http://idsapi-boe.volces.com");
        let config_path = global.config_path.as_ref().unwrap();
        let original = "[default.adrive]\nauth_mode='oauth'\nendpoint='http://idsapi-boe.volces.com'\nregion='cn-beijing'\n";
        std::fs::write(config_path, original).unwrap();
        assert_eq!(configured_share_origin(&global, None).unwrap(), None);
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), original);
        assert_eq!(
            configured_share_origin(&global, Some("https://custom.pages.test")).unwrap(),
            Some("https://custom.pages.test".into())
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn explicit_share_origin_overrides_invalid_config_before_create() {
        let fixture = Fixture::new(vec![reply(200, r#"{"ShareID":"s","InstanceID":"inst"}"#)]);
        let (global, directory) = runtime_config(&fixture.endpoint);
        let args = action(&[
            "create",
            "adrive://inst/space/a",
            "--share-endpoint",
            "https://good.test",
        ]);
        let result = manage(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(crate::domain::auth::AuthMode::Aksk),
            },
            &args,
        )
        .await;
        assert_eq!(result.unwrap()["ShareUrl"], "https://good.test/share/s");
        fixture.finish();
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn unrelated_ls_ignores_invalid_unused_share_origin() {
        #[derive(Parser)]
        struct AllCli {
            #[command(subcommand)]
            command: crate::cli::ADriveCommand,
        }
        let fixture = Fixture::new(vec![reply(
            200,
            r#"{"Files":[],"Folders":[],"IsTruncated":false}"#,
        )]);
        let (global, directory) = runtime_config(&fixture.endpoint);
        let command = AllCli::parse_from(["test", "ls", "adrive://inst/space/docs/"]).command;
        assert!(super::super::high_level::handle_high_level_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(crate::domain::auth::AuthMode::Aksk)
            },
            &command
        )
        .await
        .is_ok());
        fixture.finish();
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn chosen_invalid_share_origin_fails_before_mutation() {
        let fixture = Fixture::new(vec![reply(200, r#"{"ShareID":"s"}"#)]);
        let (global, directory) = runtime_config(&fixture.endpoint);
        let args = action(&["create", "adrive://inst/space/a"]);
        let error = manage(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(crate::domain::auth::AuthMode::Aksk),
            },
            &args,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("share_endpoint"));
        assert!(!error.to_string().contains("must-not-echo"));
        assert!(fixture.requests.lock().unwrap().is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn empty_share_target_is_rejected() {
        let ShareAction::Create(args) = action(&["create", "adrive://inst/space/   "]) else {
            panic!()
        };
        assert!(create_target(&args).is_err());
    }
    #[tokio::test]
    async fn create_unknown_outcome_has_nonretry_reconciliation_code() {
        let fixture = Fixture::new(vec![reply(503, "{}")]);
        let ShareAction::Create(args) = action(&["create", "adrive://inst/space/a"]) else {
            panic!()
        };
        let error = create_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("share_result_unknown"));
        assert_eq!(fixture.finish().len(), 1);
    }
    #[tokio::test]
    async fn incomplete_create_never_fabricates_a_share_id() {
        let fixture = Fixture::new(vec![reply(200, "{}")]);
        let ShareAction::Create(args) = action(&["create", "adrive://inst/space/a"]) else {
            panic!()
        };
        let error = create_link(
            &client(fixture.endpoint.clone()),
            &args,
            "https://pages.test",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("share_result_unknown"));
        fixture.finish();
    }
}
