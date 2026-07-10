#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod air;
mod commands;
// Engine spine is Unix-only until M7: `bossclaw-core` (bundled-SQLCipher + rustix) doesn't build
// on Windows yet, so the desktop ships without the engine there. M7 un-gates it.
#[cfg(unix)]
mod engine;
mod file_access;
mod inbox;
#[cfg(unix)]
mod integrations;
mod llm_stream;
mod markitdown;
mod secrets;
mod skills;
mod vault;
mod web_access;

use crate::commands::a2a::a2a_demo_round_trip;

use crate::air::{HttpAirClient, IdentityStore, MockAirClient};
use crate::commands::identity::*;
use std::sync::Arc;
use tauri::Manager;

fn main() {
    let mut builder = tauri::Builder::default();

    // CF2: single-instance guard. POLICY_LOCK is in-process only, so two desktop instances could
    // each run a channel + AI loop and both reserve the same budget slot (cross-process race →
    // budget bypass). Register this FIRST (Tauri 2 requirement) so a second launch hands off to the
    // running process and exits; the callback focuses the existing main window.
    //
    // M-3: `#[cfg(desktop)]` here = Tauri's `not(any(android, ios))`, which matches the dependency's
    // `cfg(any(target_os = "macos", "windows", "linux"))` predicate in Cargo.toml — both resolve to
    // "desktop only" for every supported target, so the crate is always available where this compiles.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // One-time rename migration (BossClaw -> AIR Agent). Idempotent + best-effort;
            // safe on every launch. MUST run before the vault/identity store are used below.
            let vault = vault::default_vault();
            let legacy_vault = vault::vault_for_service(vault::LEGACY_IDENTITY_SERVICE);
            let _ = air::migrate_identity_keys(legacy_vault.as_ref(), vault.as_ref());
            vault::migrate_legacy_blob_once();

            let data_dir = app.path().app_data_dir().expect("app data dir");
            let _ = air::migrate_identity_metadata(&data_dir, vault::LEGACY_IDENTITY_SERVICE);

            // On Windows there is no engine, so `data_dir` is not cloned (cloning then dropping the
            // clone would warn as unused) — move it straight into IdentityStore.
            #[cfg(unix)]
            let identity_store = IdentityStore::new(vault, data_dir.clone());
            #[cfg(not(unix))]
            let identity_store = IdentityStore::new(vault, data_dir);

            // ── Memory engine: the `bossclawd` daemon (M1a). The app is a CLIENT — the engine, the
            // embedder, the reasoner, and the evolve scheduler all live in the daemon now. ──
            //
            // Probe-then-start: connect to the daemon socket and, only if no live owner answers,
            // spawn the installed binary (the daemon's own single-owner arbitration makes an app
            // spawn racing launchd safe). This NEVER blocks/crashes boot — if the daemon can't be
            // reached the app still runs and the client surfaces `Unavailable` in the Memory UI.
            #[cfg(unix)]
            let engine = {
                let sock_path = crate::engine::daemon::resolve_socket_path(&data_dir);
                let bin_path = std::env::current_exe()
                    .map(|exe| crate::engine::daemon::resolve_bin_path(&exe))
                    .unwrap_or_else(|_| std::path::PathBuf::from("bossclawd"));
                // Stage the bundled English model into the daemon's DEFAULT resolution path
                // (`<data_dir>/models/potion-base-8M`, writable) so pull-based model resolution
                // (rung 2, I1) works WITHOUT the app pushing `BOSSCLAWD_MODEL_DIR`. The env would be
                // highest-priority and would block an opt-in multilingual language pack, so the app
                // no longer sets it — the daemon resolves its own model (signed record → staged
                // English default). The model ships read-only in the app bundle at
                // `<resource_dir>/resources/models`; staging copies English into the writable data
                // dir the first time only (idempotent). The launchd/systemd installer stages the
                // same way (O2), so both spawn paths agree without an env pin.
                let resource_models =
                    app.path().resource_dir().expect("resource dir").join("resources/models");
                let data_models_root = data_dir.join("models");
                crate::commands::engine::stage_bundled_english(&resource_models, &data_models_root);
                // `.setup` is sync (no reactor yet), so block on the bounded start attempt. Its
                // result only affects logging — a `false` still boots the app (client → Unavailable).
                tauri::async_runtime::block_on(async {
                    let _up = crate::engine::daemon::ensure_started(&sock_path, &bin_path).await;
                });
                // The transport lazily connects on first request, so it's fine to build it whether or
                // not the daemon is up yet — a not-yet-ready daemon just yields `Unavailable` until it
                // answers (and the transport reconnects transparently once it does).
                let transport: std::sync::Arc<dyn crate::engine::transport::Transport> =
                    std::sync::Arc::new(crate::engine::transport::SocketTransport::new(sock_path));
                std::sync::Arc::new(crate::engine::Engine::new(transport))
            };

            // Default to mock for dev; toggle to real AIR via AIR_AGENT_USE_REAL_AIR env var.
            // Settings UI will offer a friendlier toggle in a later task.
            let air_client: Arc<dyn air::AirClient> =
                if std::env::var("AIR_AGENT_USE_REAL_AIR").is_ok() {
                    Arc::new(HttpAirClient::production())
                } else {
                    Arc::new(MockAirClient::new())
                };

            app.manage(AppState {
                air_client,
                identity_store,
                inbox: std::sync::Arc::new(crate::inbox::manager::InboxManager::new()),
                #[cfg(unix)]
                engine,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            markitdown::md_detect,
            markitdown::md_install,
            markitdown::md_convert,
            skills::skills_list_verified,
            skills::skills_install_verified,
            file_access::file_exists,
            file_access::file_read,
            file_access::file_write,
            web_access::web_auth_set,
            web_access::web_auth_has,
            web_access::web_auth_delete,
            vault::vault_set,
            vault::vault_has,
            vault::vault_delete,
            web_access::web_fetch_public,
            web_access::web_fetch_auth,
            web_access::pw_detect,
            web_access::pw_install,
            web_access::pw_fetch_rendered,
            llm_stream::llm_plan,
            llm_stream::llm_list_models,
            llm_stream::llm_openai_compat_list_models,
            llm_stream::gemini_generate,
            llm_stream::claude_generate,
            llm_stream::llm_stream_start,
            llm_stream::llm_stream_cancel,
            is_onboarded,
            get_identity,
            get_trust_score,
            create_identity,
            rename_identity,
            check_username,
            claim_username,
            reset_identity,
            #[cfg(unix)]
            commands::engine::engine_status,
            #[cfg(unix)]
            commands::engine::engine_add_grant,
            #[cfg(unix)]
            commands::engine::engine_revoke_grant,
            #[cfg(unix)]
            commands::engine::engine_set_folder_writable,
            #[cfg(unix)]
            commands::engine::engine_list_writable,
            #[cfg(unix)]
            commands::engine::engine_set_proposals_enabled,
            #[cfg(unix)]
            commands::engine::engine_set_mandates_enabled,
            #[cfg(unix)]
            commands::engine::engine_mandates_enabled,
            #[cfg(unix)]
            commands::engine::engine_add_mandate,
            #[cfg(unix)]
            commands::engine::engine_revoke_mandate,
            #[cfg(unix)]
            commands::engine::engine_list_mandates,
            #[cfg(unix)]
            commands::engine::engine_mandate_writes,
            #[cfg(unix)]
            commands::engine::engine_list_grants,
            #[cfg(unix)]
            commands::engine::engine_run_ingest,
            #[cfg(unix)]
            commands::engine::engine_list_files,
            #[cfg(unix)]
            commands::engine::engine_list_proposals,
            #[cfg(unix)]
            commands::engine::engine_proposal_preview,
            #[cfg(unix)]
            commands::engine::engine_apply_proposal,
            #[cfg(unix)]
            commands::engine::engine_decline_proposal,
            #[cfg(unix)]
            commands::engine::engine_undo_apply,
            #[cfg(unix)]
            commands::engine::engine_pick_folder,
            #[cfg(unix)]
            commands::engine::engine_pick_file,
            #[cfg(unix)]
            commands::engine::engine_recall,
            #[cfg(unix)]
            commands::engine::engine_evolve_status,
            #[cfg(unix)]
            commands::engine::engine_set_evolve_enabled,
            #[cfg(unix)]
            commands::engine::engine_evolve_now,
            #[cfg(unix)]
            commands::engine::engine_ollama_status,
            #[cfg(unix)]
            commands::engine::engine_get_reasoner_config,
            #[cfg(unix)]
            commands::engine::engine_set_reasoner_config,
            #[cfg(unix)]
            commands::engine::engine_enable_cloud_reasoner,
            #[cfg(unix)]
            commands::engine::engine_download_language_pack,
            #[cfg(unix)]
            commands::engine::engine_set_active_model,
            #[cfg(unix)]
            commands::engine::engine_model_status,
            #[cfg(unix)]
            commands::integrations::integrations_status,
            #[cfg(unix)]
            commands::integrations::integrations_connect_claude_code,
            #[cfg(unix)]
            commands::integrations::integrations_disconnect_claude_code,
            a2a_demo_round_trip,
            commands::inbox::inbox_status,
            commands::inbox::inbox_identity,
            commands::inbox::inbox_start,
            commands::inbox::inbox_stop,
            commands::inbox::inbox_send,
            commands::inbox::inbox_conversations,
            commands::inbox::inbox_contacts,
            commands::inbox::inbox_history,
            commands::inbox::inbox_policy_get,
            commands::inbox::inbox_policy_set,
            commands::inbox::inbox_ai_reserve,
            commands::inbox::inbox_ai_confirm,
            commands::inbox::inbox_ai_cancel,
            commands::inbox::inbox_default_agent,
            inbox::channel::inbox_channel_start,
            inbox::channel::inbox_channel_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
