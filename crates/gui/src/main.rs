mod window;

use adw::prelude::*;

pub const APP_ID: &str = "io.github.DJPoulter.YACCGL";

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        // Single window: re-activating (e.g. a second launch) just focuses it.
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        window::build(app);
    });
    app.run()
}
