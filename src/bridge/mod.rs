//! The Tauri shell: the managed state, the command surface, and the builder
//! that starts the window.
//!
//! This is the only module that mentions Tauri. Everything it calls lives in
//! [`crate::services`], which knows nothing about a webview.

pub mod commands;
pub mod dto;
pub mod state;

use tauri::Manager;

use state::AppState;

pub fn run() {
    crate::services::logging::init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::load())
        .setup(|app| {
            // The state is created before the window so the synchronous
            // database reads happen while nothing is on screen. It needs an
            // `AppHandle` only to emit.
            let state = app.state::<AppState>().inner().clone();
            state.attach(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::activity::clear_activity,
            commands::activity::recent_log_lines,
            commands::activity::current_log_level,
            commands::app::bootstrap,
            commands::app::open_url,
            commands::app::check_for_updates,
            commands::app::set_window_theme,
            commands::app::system_theme,
            commands::app::quit,
            commands::connection::get_state,
            commands::connection::check_token,
            commands::connection::preview_zones,
            commands::connection::connect,
            commands::connection::restore_session,
            commands::connection::disconnect,
            commands::tunnels::refresh_tunnels,
            commands::tunnels::review_tunnel,
            commands::tunnels::create_tunnel,
            commands::tunnels::rename_tunnel,
            commands::tunnels::delete_tunnel_impact,
            commands::tunnels::delete_tunnel,
            commands::tunnels::disconnect_connector,
            commands::tunnels::list_connectors,
            commands::tunnels::fetch_run_token,
            commands::hostnames::load_hostnames,
            commands::hostnames::load_all_hostnames,
            commands::hostnames::publish_hostname,
            commands::hostnames::unpublish_hostname,
            commands::hostnames::disable_hostname,
            commands::hostnames::enable_hostname,
            commands::hostnames::repair_dns,
            commands::hostnames::rollback_ingress,
            commands::zones::refresh_zones,
            commands::hosts::refresh_hosts,
            commands::hosts::save_host,
            commands::hosts::update_host,
            commands::hosts::delete_host,
            commands::hosts::probe_host_key,
            commands::hosts::trust_host_key,
            commands::hosts::preflight_host,
            commands::hosts::host_service_action,
            commands::hosts::host_logs,
            commands::hosts::read_transport,
            commands::hosts::set_transport,
            commands::hosts::plan_deployment,
            commands::hosts::apply_deployment,
            commands::hosts::manual_setup_instructions,
            commands::hosts::sudoers_snippet,
            commands::hosts::save_host_passphrase,
            commands::hosts::delete_host_passphrase,
            commands::hosts::local_inspect,
            commands::hosts::local_service_action,
            commands::hosts::local_logs,
            commands::hosts::local_set_transport,
            commands::settings::update_settings,
            commands::settings::set_log_level,
            commands::settings::diagnostic_report,
        ]);

    if let Err(error) = builder.run(tauri::generate_context!()) {
        eprintln!("tunnel-manager: {error}");
        std::process::exit(1);
    }
}
