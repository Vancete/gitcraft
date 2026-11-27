// No console window behind the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::Ordering;

use gitcraft::app;

/// eframe asks the GPU for wgpu's default limits, which some drivers don't reach (Mali's
/// PanVK allows 3D textures of 512, not 2048) and the window then never opens. Ask only for
/// what the adapter has, with eframe's 8192 for 2D textures where it can do that much.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu;
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.device_descriptor = std::sync::Arc::new(|adapter| {
        let supported = adapter.limits();
        eframe::wgpu::DeviceDescriptor {
            label: Some("egui wgpu device"),
            required_limits: eframe::wgpu::Limits {
                max_texture_dimension_2d: supported.max_texture_dimension_2d.min(8192),
                ..supported
            },
            ..Default::default()
        }
    });
    egui_wgpu::WgpuConfiguration {
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

fn main() -> eframe::Result {
    // Started by ssh to ask the user something (see `askpass`): ask the window and exit.
    if let Some(code) = gitcraft::askpass::run_if_asked() {
        std::process::exit(code);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("GitCraft")
            // Matches gitcraft.desktop, so Linux desktops show its icon and group its windows.
            .with_app_id("gitcraft")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(app::window_icon()),
        // Runs after eframe restored the last session's size and position.
        window_builder: Some(Box::new(|builder| {
            if cfg!(windows) && builder.maximized == Some(true) {
                app::MAXIMIZE_WHEN_SHOWN.store(true, Ordering::Relaxed);
                builder.with_maximized(false)
            } else {
                builder
            }
        })),
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        "gitcraft",
        options,
        Box::new(|cc| {
            let open_arg = std::env::args_os().nth(1).map(std::path::PathBuf::from);
            Ok(Box::new(app::App::new(cc, open_arg)))
        }),
    )
}
