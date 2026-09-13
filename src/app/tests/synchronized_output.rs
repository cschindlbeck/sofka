use super::*;
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};
use std::{cell::RefCell, io, rc::Rc};

const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    flushed: usize,
    fail_flush: bool,
}

struct Writer(Rc<RefCell<Output>>);

impl io::Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut output = self.0.borrow_mut();
        if std::mem::take(&mut output.fail_flush) {
            return Err(io::Error::other("draw flush failed"));
        }
        output.flushed = output.bytes.len();
        Ok(())
    }
}

fn terminal(output: &Rc<RefCell<Output>>) -> Terminal<CrosstermBackend<Writer>> {
    Terminal::with_options(
        CrosstermBackend::new(Writer(Rc::clone(output))),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 80, 16)),
        },
    )
    .unwrap()
}

#[tokio::test]
async fn held_down_keeps_each_viewport_update_inside_sync_markers() {
    let (mut app, _rx) = test_app();
    for i in 0..40 {
        apply(
            &mut app,
            json!({
                "apiVersion": "v1", "kind": "Pod",
                "metadata": {"name": format!("pod-{i:02}"), "namespace": "default"},
                "status": {"phase": "Running"}
            }),
        );
    }
    let output = Rc::new(RefCell::new(Output::default()));
    let mut terminal = terminal(&output);
    crate::ui::present(&mut terminal, &mut app).unwrap();
    for _ in 0..30 {
        *output.borrow_mut() = Output::default();
        app.handle_key(press(KeyCode::Down)).unwrap();
        crate::ui::present(&mut terminal, &mut app).unwrap();
        let output = output.borrow();
        assert!(output.bytes.starts_with(BEGIN));
        assert!(output.bytes.ends_with(END));
        assert_eq!(output.flushed, output.bytes.len());
        assert!(output.bytes.len() > BEGIN.len() + END.len());
        assert_eq!(
            output
                .bytes
                .windows(BEGIN.len())
                .filter(|s| *s == BEGIN)
                .count(),
            1
        );
        assert_eq!(
            output
                .bytes
                .windows(END.len())
                .filter(|s| *s == END)
                .count(),
            1
        );
    }
    assert!(app.table_state.offset() > 0);
    assert!(app.table_state.selected().unwrap() >= app.table_page_rows);
}

#[tokio::test]
async fn failed_draw_still_ends_and_flushes_sync_update() {
    let (mut app, _rx) = test_app();
    let output = Rc::new(RefCell::new(Output {
        fail_flush: true,
        ..Output::default()
    }));
    let mut terminal = terminal(&output);
    app.handle_key(press(KeyCode::Down)).unwrap();
    let error = crate::ui::present(&mut terminal, &mut app).unwrap_err();
    assert_eq!(error.to_string(), "draw flush failed");
    let output = output.borrow();
    assert!(output.bytes.starts_with(BEGIN));
    assert!(output.bytes.ends_with(END));
    assert_eq!(output.flushed, output.bytes.len());
}
