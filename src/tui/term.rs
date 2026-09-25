use super::app::App;
use super::view;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{FromRawFd, RawFd};

// the tui draws on /dev/tty while the process's own stdin is /dev/null and stdout and stderr feed a pipe,
// so everything commands print lands in the output pane instead of on the screen
pub struct Term {
    terminal: Terminal<CrosstermBackend<File>>,
    tty: File,
    saved: [RawFd; 3],
    pipe_write: RawFd,
}

fn check(result: i32) -> io::Result<i32> {
    if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

impl Term {
    // takes over the terminal; the returned file is the read end of what commands print
    pub fn enter() -> io::Result<(Term, File)> {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let mut fds = [0; 2];
        // SAFETY: plain fd syscalls on descriptors this process owns
        let saved = unsafe {
            check(libc::pipe(fds.as_mut_ptr()))?;
            [check(libc::dup(0))?, check(libc::dup(1))?, check(libc::dup(2))?]
        };
        let terminal = Terminal::new(CrosstermBackend::new(tty.try_clone()?))?;
        let mut term = Term { terminal, tty, saved, pipe_write: fds[1] };
        term.redirect()?;

        // a failure halfway must give the terminal back, or the error would vanish into the pipe
        if let Err(error) = term.take_screen() {
            let _ = term.suspend();
            return Err(error);
        }
        // SAFETY: fds[0] is the pipe's read end, owned from here on by the returned file
        Ok((term, unsafe { File::from_raw_fd(fds[0]) }))
    }

    // stdin from /dev/null, stdout and stderr into the pipe
    fn redirect(&self) -> io::Result<()> {
        let null = OpenOptions::new().read(true).open("/dev/null")?;
        // SAFETY: dup2 onto the standard descriptors, which the process owns
        unsafe {
            check(libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&null), 0))?;
            check(libc::dup2(self.pipe_write, 1))?;
            check(libc::dup2(self.pipe_write, 2))?;
        }
        Ok(())
    }

    // the real stdin, stdout, and stderr back
    fn restore(&self) -> io::Result<()> {
        // SAFETY: the saved descriptors are duplicates of the originals, still open
        unsafe {
            (0..3).try_for_each(|fd| check(libc::dup2(self.saved[fd as usize], fd)).map(|_| ()))?;
        }
        Ok(())
    }

    // raw mode, the alternate screen, and a fresh terminal so the next frame redraws everything;
    // ratatui's own clear would ask the terminal for the cursor position, which not every terminal answers
    fn take_screen(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(self.tty, EnterAlternateScreen, EnableMouseCapture, Hide, Clear(ClearType::All))?;
        self.terminal = Terminal::new(CrosstermBackend::new(self.tty.try_clone()?))?;
        Ok(())
    }

    // hands the terminal back, e.g. for the editor
    pub fn suspend(&mut self) -> io::Result<()> {
        execute!(self.tty, DisableMouseCapture, LeaveAlternateScreen, Show)?;
        disable_raw_mode()?;
        self.restore()
    }

    // takes it again after suspend
    pub fn resume(&mut self) -> io::Result<()> {
        self.redirect()?;
        self.take_screen()
    }

    pub fn draw(&mut self, app: &App) -> io::Result<()> {
        self.terminal.draw(|frame| view::draw(frame, app)).map(|_| ())
    }
}
