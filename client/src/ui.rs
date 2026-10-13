use std::{
    collections::HashMap,
    iter::FromIterator,
    sync::{Arc, Mutex},
};

use sciter::Value;

use hbb_common::{
    allow_err,
    config::{LocalConfig, PeerConfig},
    log,
};

#[cfg(not(feature = "flutter"))]
use crate::ui_session_interface::Session;
use crate::{common::get_app_name, ipc, ui_interface::*};

mod cm;
#[cfg(feature = "inline")]
pub mod inline;
pub mod remote;

#[allow(dead_code)]
type Status = (i32, bool, i64, String);

lazy_static::lazy_static! {
    // stupid workaround for https://sciter.com/forums/topic/crash-on-latest-tis-mac-sdk-sometimes/
    static ref STUPID_VALUES: Mutex<Vec<Arc<Vec<Value>>>> = Default::default();
}

#[cfg(not(feature = "flutter"))]
lazy_static::lazy_static! {
    pub static ref CUR_SESSION: Arc<Mutex<Option<Session<remote::SciterHandler>>>> = Default::default();
}

struct UIHostHandler;

pub fn start(args: &mut [String]) {
    #[cfg(target_os = "macos")]
    crate::platform::delegate::show_dock();
    #[cfg(all(target_os = "linux", feature = "inline"))]
    {
        let app_dir = std::env::var("APPDIR").unwrap_or("".to_string());
        let mut so_path = "/usr/share/vibedesk/libsciter-gtk.so".to_owned();
        for (prefix, dir) in [
            ("", "/usr"),
            ("", "/app"),
            (&app_dir, "/usr"),
            (&app_dir, "/app"),
        ]
        .iter()
        {
            let path = format!("{prefix}{dir}/share/vibedesk/libsciter-gtk.so");
            if std::path::Path::new(&path).exists() {
                so_path = path;
                break;
            }
        }
        sciter::set_library(&so_path).ok();
    }
    #[cfg(windows)]
    // Check if there is a sciter.dll nearby.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sciter_dll_path = parent.join("sciter.dll");
            if sciter_dll_path.exists() {
                // Try to set the sciter dll.
                let p = sciter_dll_path.to_string_lossy().to_string();
                log::debug!("Found dll:{}, \n {:?}", p, sciter::set_library(&p));
            }
        }
    }
    // https://github.com/c-smile/sciter-sdk/blob/master/include/sciter-x-types.h
    // https://github.com/rustdesk/rustdesk/issues/132#issuecomment-886069737
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::GfxLayer(
        sciter::GFX_LAYER::WARP
    )));
    use sciter::SCRIPT_RUNTIME_FEATURES::*;
    allow_err!(sciter::set_options(sciter::RuntimeOptions::ScriptFeatures(
        ALLOW_FILE_IO as u8 | ALLOW_SOCKET_IO as u8 | ALLOW_EVAL as u8 | ALLOW_SYSINFO as u8
    )));
    let mut frame = sciter::WindowBuilder::main_window().create();
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::UxTheming(true)));
    frame.set_title(&crate::get_app_name());
    #[cfg(target_os = "macos")]
    crate::platform::delegate::make_menubar(frame.get_host(), args.is_empty());
    #[cfg(windows)]
    crate::platform::try_set_window_foreground(frame.get_hwnd() as _);
    let page;
    if args.len() > 1 && args[0] == "--play" {
        args[0] = "--connect".to_owned();
        let path: std::path::PathBuf = (&args[1]).into();
        let id = path
            .file_stem()
            .map(|p| p.to_str().unwrap_or(""))
            .unwrap_or("")
            .to_owned();
        args[1] = id;
    }
    if args.is_empty() {
        std::thread::spawn(move || check_zombie());
        crate::common::check_software_update();
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "index.html";
        // Start pulse audio local server.
        #[cfg(target_os = "linux")]
        std::thread::spawn(crate::ipc::start_pa);
    } else if args[0] == "--install" {
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "install.html";
    } else if args[0] == "--cm" {
        frame.register_behavior("connection-manager", move || {
            Box::new(cm::SciterConnectionManager::new())
        });
        page = "cm.html";
        *cm::HIDE_CM.lock().unwrap() = crate::ipc::get_config("hide_cm")
            .ok()
            .flatten()
            .unwrap_or_default()
            == "true";
    } else if (args[0] == "--connect"
        || args[0] == "--file-transfer"
        || args[0] == "--port-forward"
        || args[0] == "--rdp")
        && args.len() > 1
    {
        #[cfg(windows)]
        {
            let hw = frame.get_host().get_hwnd();
            crate::platform::windows::enable_lowlevel_keyboard(hw as _);
        }
        let mut iter = args.iter();
        let Some(cmd) = iter.next() else {
            log::error!("Failed to get cmd arg");
            return;
        };
        let cmd = cmd.to_owned();
        let Some(id) = iter.next() else {
            log::error!("Failed to get id arg");
            return;
        };
        let id = id.to_owned();
        let pass = iter.next().unwrap_or(&"".to_owned()).clone();
        let args: Vec<String> = iter.map(|x| x.clone()).collect();
        frame.set_title(&id);
        frame.register_behavior("native-remote", move || {
            let handler =
                remote::SciterSession::new(cmd.clone(), id.clone(), pass.clone(), args.clone());
            #[cfg(not(feature = "flutter"))]
            {
                *CUR_SESSION.lock().unwrap() = Some(handler.inner());
            }
            Box::new(handler)
        });
        page = "remote.html";
    } else {
        log::error!("Wrong command: {:?}", args);
        return;
    }
    #[cfg(feature = "inline")]
    {
        let html = if page == "index.html" {
            inline::get_index()
        } else if page == "cm.html" {
            inline::get_cm()
        } else if page == "install.html" {
            inline::get_install()
        } else {
            inline::get_remote()
        };
        frame.load_html(html.as_bytes(), Some(page));
    }
    #[cfg(not(feature = "inline"))]
    frame.load_file(&format!(
        "file://{}/src/ui/{}",
        std::env::current_dir()
            .map(|c| c.display().to_string())
            .unwrap_or("".to_owned()),
        page
    ));
    let hide_cm = *cm::HIDE_CM.lock().unwrap();
    if !args.is_empty() && args[0] == "--cm" && hide_cm {
        // run_app calls expand(show) + run_loop, we use collapse(hide) + run_loop instead to create a hidden window
        frame.collapse(true);
        frame.run_loop();
        return;
    }
    frame.run_app();
}

struct UI {}

impl UI {
    fn recent_sessions_updated(&self) -> bool {
        recent_sessions_updated()
    }

    fn get_id(&self) -> String {
        ipc::get_id()
    }

    fn temporary_password(&mut self) -> String {
        temporary_password()
    }

    fn update_temporary_password(&self) {
        update_temporary_password()
    }

    fn set_permanent_password(&self, password: String) {
        let _ = set_permanent_password_with_result(password);
    }

    fn is_local_permanent_password_set(&self) -> bool {
        is_local_permanent_password_set()
    }

    fn is_permanent_password_set(&self) -> bool {
        is_permanent_password_set()
    }

    fn get_remote_id(&mut self) -> String {
        LocalConfig::get_remote_id()
    }

    fn set_remote_id(&mut self, id: String) {
        LocalConfig::set_remote_id(&id);
    }

    fn goto_install(&mut self) {
        goto_install();
    }

    fn install_me(&mut self, _options: String, _path: String) {
        install_me(_options, _path, false, false);
    }

    fn update_me(&self, _path: String) {
        update_me(_path);
    }

    fn run_without_install(&self) {
        run_without_install();
    }

    fn show_run_without_install(&self) -> bool {
        show_run_without_install()
    }

    fn get_license(&self) -> String {
        get_license()
    }

    fn get_option(&self, key: String) -> String {
        get_option(key)
    }

    fn get_local_option(&self, key: String) -> String {
        get_local_option(key)
    }

    fn set_local_option(&self, key: String, value: String) {
        set_local_option(key, value);
    }

    fn peer_has_password(&self, id: String) -> bool {
        peer_has_password(id)
    }

    fn forget_password(&self, id: String) {
        forget_password(id)
    }

    fn get_peer_option(&self, id: String, name: String) -> String {
        get_peer_option(id, name)
    }

    fn set_peer_option(&self, id: String, name: String, value: String) {
        set_peer_option(id, name, value)
    }

    fn using_public_server(&self) -> bool {
        crate::using_public_server()
    }

    fn is_incoming_only(&self) -> bool {
        hbb_common::config::is_incoming_only()
    }

    pub fn is_outgoing_only(&self) -> bool {
        hbb_common::config::is_outgoing_only()
    }

    pub fn is_custom_client(&self) -> bool {
        crate::common::is_custom_client()
    }

    pub fn is_disable_settings(&self) -> bool {
        hbb_common::config::is_disable_settings()
    }

    pub fn is_disable_account(&self) -> bool {
        hbb_common::config::is_disable_account()
    }

    pub fn is_disable_installation(&self) -> bool {
        hbb_common::config::is_disable_installation()
    }

    pub fn is_disable_ab(&self) -> bool {
        hbb_common::config::is_disable_ab()
    }

    fn get_options(&self) -> Value {
        let hashmap: HashMap<String, String> =
            serde_json::from_str(&get_options()).unwrap_or_default();
        let mut m = Value::map();
        for (k, v) in hashmap {
            m.set_item(k, v);
        }
        m
    }

    fn test_if_valid_server(&self, host: String, test_with_proxy: bool) -> String {
        test_if_valid_server(host, test_with_proxy)
    }

    fn get_sound_inputs(&self) -> Value {
        Value::from_iter(get_sound_inputs())
    }

    fn set_options(&self, v: Value) {
        let mut m = HashMap::new();
        for (k, v) in v.items() {
            if let Some(k) = k.as_string() {
                if let Some(v) = v.as_string() {
                    if !v.is_empty() {
                        m.insert(k, v);
                    }
                }
            }
        }
        set_options(m);
    }

    fn set_option(&self, key: String, value: String) {
        set_option(key, value);
    }

    fn install_path(&mut self) -> String {
        install_path()
    }

    fn install_options(&self) -> String {
        install_options()
    }

    fn get_socks(&self) -> Value {
        Value::from_iter(get_socks())
    }

    fn set_socks(&self, proxy: String, username: String, password: String) {
        set_socks(proxy, username, password)
    }

    fn is_installed(&self) -> bool {
        is_installed()
    }

    fn get_supported_privacy_mode_impls(&self) -> String {
        serde_json::to_string(&crate::privacy_mode::get_supported_privacy_mode_impl())
            .unwrap_or_default()
    }

    fn is_root(&self) -> bool {
        is_root()
    }

    fn is_release(&self) -> bool {
        #[cfg(not(debug_assertions))]
        return true;
        #[cfg(debug_assertions)]
        return false;
    }

    fn is_share_rdp(&self) -> bool {
        is_share_rdp()
    }

    fn set_share_rdp(&self, _enable: bool) {
        set_share_rdp(_enable);
    }

    fn is_installed_lower_version(&self) -> bool {
        is_installed_lower_version()
    }

    fn closing(&mut self, x: i32, y: i32, w: i32, h: i32) {
        crate::server::input_service::fix_key_down_timeout_at_exit();
        LocalConfig::set_size(x, y, w, h);
    }

    fn get_size(&mut self) -> Value {
        let s = LocalConfig::get_size();
        let mut v = Vec::new();
        v.push(s.0);
        v.push(s.1);
        v.push(s.2);
        v.push(s.3);
        Value::from_iter(v)
    }

    fn get_mouse_time(&self) -> f64 {
        get_mouse_time()
    }

    fn check_mouse_time(&self) {
        check_mouse_time()
    }

    fn get_connect_status(&mut self) -> Value {
        let mut v = Value::array(0);
        let x = get_connect_status();
        v.push(x.status_num);
        v.push(x.key_confirmed);
        v.push(x.id);
        v
    }

    #[inline]
    fn get_peer_value(id: String, p: PeerConfig) -> Value {
        let values = vec![
            id,
            p.info.username.clone(),
            p.info.hostname.clone(),
            p.info.platform.clone(),
            p.options.get("alias").unwrap_or(&"".to_owned()).to_owned(),
        ];
        Value::from_iter(values)
    }

    fn get_peer(&self, id: String) -> Value {
        let c = get_peer(id.clone());
        Self::get_peer_value(id, c)
    }

    fn get_fav(&self) -> Value {
        Value::from_iter(get_fav())
    }

    fn store_fav(&self, fav: Value) {
        let mut tmp = vec![];
        fav.values().for_each(|v| {
            if let Some(v) = v.as_string() {
                if !v.is_empty() {
                    tmp.push(v);
                }
            }
        });
        store_fav(tmp);
    }

    fn get_recent_sessions(&mut self) -> Value {
        // to-do: limit number of recent sessions, and remove old peer file
        let peers: Vec<Value> = PeerConfig::peers(None)
            .drain(..)
            .map(|p| Self::get_peer_value(p.0, p.2))
            .collect();
        Value::from_iter(peers)
    }

    fn get_icon(&mut self) -> String {
        get_icon()
    }

    fn remove_peer(&mut self, id: String) {
        PeerConfig::remove(&id);
    }

    fn remove_discovered(&mut self, id: String) {
        remove_discovered(id);
    }

    fn send_wol(&mut self, id: String) {
        crate::lan::send_wol(id)
    }

    fn new_remote(&mut self, id: String, remote_type: String, force_relay: bool) {
        new_remote(id, remote_type, force_relay)
    }

    fn is_process_trusted(&mut self, _prompt: bool) -> bool {
        is_process_trusted(_prompt)
    }

    fn is_can_screen_recording(&mut self, _prompt: bool) -> bool {
        is_can_screen_recording(_prompt)
    }

    fn is_installed_daemon(&mut self, _prompt: bool) -> bool {
        is_installed_daemon(_prompt)
    }

    fn get_error(&mut self) -> String {
        get_error()
    }

    fn is_login_wayland(&mut self) -> bool {
        is_login_wayland()
    }

    fn current_is_wayland(&mut self) -> bool {
        current_is_wayland()
    }

    fn get_software_update_url(&self) -> String {
        crate::SOFTWARE_UPDATE_URL.lock().unwrap().clone()
    }

    fn get_new_version(&self) -> String {
        get_new_version()
    }

    fn get_version(&self) -> String {
        get_version()
    }

    fn get_fingerprint(&self) -> String {
        get_fingerprint()
    }

    fn get_app_name(&self) -> String {
        get_app_name()
    }

    fn get_software_ext(&self) -> String {
        #[cfg(windows)]
        let p = "exe";
        #[cfg(target_os = "macos")]
        let p = "dmg";
        #[cfg(target_os = "linux")]
        let p = "deb";
        p.to_owned()
    }

    fn get_software_store_path(&self) -> String {
        let mut p = std::env::temp_dir();
        let name = crate::SOFTWARE_UPDATE_URL
            .lock()
            .unwrap()
            .split("/")
            .last()
            .map(|x| x.to_owned())
            .unwrap_or(crate::get_app_name());
        p.push(name);
        format!("{}.{}", p.to_string_lossy(), self.get_software_ext())
    }

    fn create_shortcut(&self, _id: String) {
        #[cfg(windows)]
        create_shortcut(_id)
    }

    fn discover(&self) {
        std::thread::spawn(move || {
            allow_err!(crate::lan::discover());
        });
    }

    fn get_lan_peers(&self) -> String {
        // let peers = get_lan_peers()
        //     .into_iter()
        //     .map(|mut peer| {
        //         (
        //             peer.remove("id").unwrap_or_default(),
        //             peer.remove("username").unwrap_or_default(),
        //             peer.remove("hostname").unwrap_or_default(),
        //             peer.remove("platform").unwrap_or_default(),
        //         )
        //     })
        //     .collect::<Vec<(String, String, String, String)>>();
        serde_json::to_string(&get_lan_peers()).unwrap_or_default()
    }

    fn get_uuid(&self) -> String {
        get_uuid()
    }

    fn open_url(&self, url: String) {
        #[cfg(windows)]
        let p = "explorer";
        #[cfg(target_os = "macos")]
        let p = "open";
        #[cfg(target_os = "linux")]
        let p = if std::path::Path::new("/usr/bin/firefox").exists() {
            "firefox"
        } else {
            "xdg-open"
        };
        allow_err!(std::process::Command::new(p).arg(url).spawn());
    }

    fn change_id(&self, id: String) {
        reset_async_job_status();
        let old_id = self.get_id();
        change_id_shared(id, old_id);
    }

    fn http_request(&self, url: String, method: String, body: Option<String>, header: String) {
        http_request(url, method, body, header)
    }

    fn post_request(&self, url: String, body: String, header: String) {
        post_request(url, body, header)
    }

    fn is_ok_change_id(&self) -> bool {
        hbb_common::machine_uid::get().is_ok()
    }

    fn get_async_job_status(&self) -> String {
        get_async_job_status()
    }

    fn get_http_status(&self, url: String) -> Option<String> {
        get_async_http_status(url)
    }

    fn t(&self, name: String) -> String {
        crate::client::translate(name)
    }

    fn is_xfce(&self) -> bool {
        crate::platform::is_xfce()
    }

    fn get_api_server(&self) -> String {
        get_api_server()
    }

    fn has_hwcodec(&self) -> bool {
        has_hwcodec()
    }

    fn has_vram(&self) -> bool {
        has_vram()
    }

    fn get_langs(&self) -> String {
        get_langs()
    }

    fn video_save_directory(&self, root: bool) -> String {
        video_save_directory(root)
    }

    fn handle_relay_id(&self, id: String) -> String {
        handle_relay_id(&id).to_owned()
    }

    fn get_login_device_info(&self) -> String {
        get_login_device_info_json()
    }

    fn support_remove_wallpaper(&self) -> bool {
        support_remove_wallpaper()
    }

    fn has_valid_2fa(&self) -> bool {
        has_valid_2fa()
    }

    fn generate2fa(&self) -> String {
        generate2fa()
    }

    pub fn verify2fa(&self, code: String) -> bool {
        verify2fa(code)
    }

    fn verify_login(&self, raw: String, id: String) -> bool {
        crate::verify_login(&raw, &id)
    }

    fn generate_2fa_img_src(&self, data: String) -> String {
        let v = qrcode_generator::to_png_to_vec(data, qrcode_generator::QrCodeEcc::Low, 128)
            .unwrap_or_default();
        let s = hbb_common::sodiumoxide::base64::encode(
            v,
            hbb_common::sodiumoxide::base64::Variant::Original,
        );
        format!("data:image/png;base64,{s}")
    }

    pub fn check_hwcodec(&self) {
        check_hwcodec()
    }

    fn is_option_fixed(&self, key: String) -> bool {
        crate::ui_interface::is_option_fixed(&key)
    }

    fn get_builtin_option(&self, key: String) -> String {
        crate::ui_interface::get_builtin_option(&key)
    }

    fn is_remote_modify_enabled_by_control_permissions(&self) -> String {
        match crate::ui_interface::is_remote_modify_enabled_by_control_permissions() {
            Some(true) => "true",
            Some(false) => "false",
            None => "",
        }
        .to_string()
    }
}

impl sciter::EventHandler for UI {
    sciter::dispatch_script_call! {
        fn t(String);
        fn get_api_server();
        fn is_xfce();
        fn using_public_server();
        fn is_custom_client();
        fn is_outgoing_only();
        fn is_incoming_only();
        fn is_disable_settings();
        fn is_disable_account();
        fn is_disable_installation();
        fn is_disable_ab();
        fn get_id();
        fn temporary_password();
        fn update_temporary_password();
        fn set_permanent_password(String);
        fn is_local_permanent_password_set();
        fn is_permanent_password_set();
        fn get_remote_id();
        fn set_remote_id(String);
        fn closing(i32, i32, i32, i32);
        fn get_size();
        fn new_remote(String, String, bool);
        fn send_wol(String);
        fn remove_peer(String);
        fn remove_discovered(String);
        fn get_connect_status();
        fn get_mouse_time();
        fn check_mouse_time();
        fn get_recent_sessions();
        fn get_peer(String);
        fn get_fav();
        fn store_fav(Value);
        fn recent_sessions_updated();
        fn get_icon();
        fn install_me(String, String);
        fn is_installed();
        fn get_supported_privacy_mode_impls();
        fn is_root();
        fn is_release();
        fn set_socks(String, String, String);
        fn get_socks();
        fn is_share_rdp();
        fn set_share_rdp(bool);
        fn is_installed_lower_version();
        fn install_path();
        fn install_options();
        fn goto_install();
        fn is_process_trusted(bool);
        fn is_can_screen_recording(bool);
        fn is_installed_daemon(bool);
        fn get_error();
        fn is_login_wayland();
        fn current_is_wayland();
        fn get_options();
        fn get_option(String);
        fn get_local_option(String);
        fn set_local_option(String, String);
        fn get_peer_option(String, String);
        fn peer_has_password(String);
        fn forget_password(String);
        fn set_peer_option(String, String, String);
        fn get_license();
        fn test_if_valid_server(String, bool);
        fn get_sound_inputs();
        fn set_options(Value);
        fn set_option(String, String);
        fn get_software_update_url();
        fn get_new_version();
        fn get_version();
        fn get_fingerprint();
        fn update_me(String);
        fn show_run_without_install();
        fn run_without_install();
        fn get_app_name();
        fn get_software_store_path();
        fn get_software_ext();
        fn open_url(String);
        fn change_id(String);
        fn get_async_job_status();
        fn post_request(String, String, String);
        fn is_ok_change_id();
        fn create_shortcut(String);
        fn discover();
        fn get_lan_peers();
        fn get_uuid();
        fn has_hwcodec();
        fn has_vram();
        fn get_langs();
        fn video_save_directory(bool);
        fn handle_relay_id(String);
        fn get_login_device_info();
        fn support_remove_wallpaper();
        fn has_valid_2fa();
        fn generate2fa();
        fn generate_2fa_img_src(String);
        fn verify2fa(String);
        fn check_hwcodec();
        fn verify_login(String, String);
        fn is_option_fixed(String);
        fn get_builtin_option(String);
        fn is_remote_modify_enabled_by_control_permissions();
    }
}

impl sciter::host::HostHandler for UIHostHandler {
    fn on_graphics_critical_failure(&mut self) {
        log::error!("Critical rendering error: e.g. DirectX gfx driver error. Most probably bad gfx drivers.");
    }
}

#[cfg(not(target_os = "linux"))]
fn get_sound_inputs() -> Vec<String> {
    let mut out = Vec::new();
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Ok(devices) = host.devices() {
        for device in devices {
            if device.default_input_config().is_err() {
                continue;
            }
            if let Ok(name) = device.name() {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn get_sound_inputs() -> Vec<String> {
    crate::platform::linux::get_pa_sources()
        .drain(..)
        .map(|x| x.1)
        .collect()
}

// sacrifice some memory
pub fn value_crash_workaround(values: &[Value]) -> Arc<Vec<Value>> {
    let persist = Arc::new(values.to_vec());
    STUPID_VALUES.lock().unwrap().push(persist.clone());
    persist
}

pub fn get_icon() -> String {
    // 128x128
    #[cfg(target_os = "macos")]
    // 128x128 on 160x160 canvas, then shrink to 128, mac looks better with padding
    {
        "data:image/png;base64,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".into()
    }
    #[cfg(not(target_os = "macos"))] // 128x128 no padding
    {
        "data:image/png;base64,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".into()
    }
}
