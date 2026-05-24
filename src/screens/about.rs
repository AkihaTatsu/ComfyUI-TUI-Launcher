//! About screen showing the application name, version, and homepage link.

use crate::core::{i18n, theme};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Renders the About screen into `area`.
pub fn render(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(vec![Span::styled(i18n::t("app_title"), theme::accent())]),
        Line::from(format!(
            "{}: {}",
            i18n::t("about_version"),
            env!("CARGO_PKG_VERSION")
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("{}: ", i18n::t("about_homepage")), theme::base()),
            Span::raw("https://github.com/AkihaTatsu/ComfyUI-TUI-Launcher"),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme::border()),
        ),
        area,
    );
}
