use std::{io::{LineWriter, stderr}, time::Duration};

use anyhow::{Result, bail};
use crossterm::{cursor::{RestorePosition, SavePosition}, execute, style::Print, terminal::{disable_raw_mode, enable_raw_mode}};
use scopeguard::defer;
use tokio::{io::{AsyncReadExt, BufReader}, time::timeout};
use tracing::{error, warn};
use yazi_shared::env_exists;

use crate::{Adapter, Mux, TMUX};

#[derive(Clone, Debug)]
pub enum Emulator {
	Unknown(Vec<Adapter>),
	Kitty,
	Konsole,
	Iterm2,
	WezTerm,
	Foot,
	Ghostty,
	Microsoft,
	Rio,
	BlackBox,
	VSCode,
	Tabby,
	Hyper,
	Mintty,
	Neovim,
	Apple,
	Urxvt,
}

impl Emulator {
	pub fn adapters(self) -> Vec<Adapter> {
		match self {
			Self::Unknown(adapters) => adapters,
			Self::Kitty => vec![Adapter::Kgp],
			Self::Konsole => vec![Adapter::KgpOld],
			Self::Iterm2 => vec![Adapter::Iip, Adapter::Sixel],
			Self::WezTerm => vec![Adapter::Iip, Adapter::Sixel],
			Self::Foot => vec![Adapter::Sixel],
			Self::Ghostty => vec![Adapter::Kgp],
			Self::Microsoft => vec![Adapter::Sixel],
			Self::Rio => vec![Adapter::Iip, Adapter::Sixel],
			Self::BlackBox => vec![Adapter::Sixel],
			Self::VSCode => vec![Adapter::Iip, Adapter::Sixel],
			Self::Tabby => vec![Adapter::Iip, Adapter::Sixel],
			Self::Hyper => vec![Adapter::Iip, Adapter::Sixel],
			Self::Mintty => vec![Adapter::Iip],
			Self::Neovim => vec![],
			Self::Apple => vec![],
			Self::Urxvt => vec![],
		}
	}
}

impl Emulator {
	pub fn detect() -> Self {
		if env_exists("NVIM_LOG_FILE") && env_exists("NVIM") {
			return Self::Neovim;
		}

		let vars = [
			("KITTY_WINDOW_ID", Self::Kitty),
			("KONSOLE_VERSION", Self::Konsole),
			("ITERM_SESSION_ID", Self::Iterm2),
			("WEZTERM_EXECUTABLE", Self::WezTerm),
			("GHOSTTY_RESOURCES_DIR", Self::Ghostty),
			("WT_Session", Self::Microsoft),
			("VSCODE_INJECTION", Self::VSCode),
			("TABBY_CONFIG_DIRECTORY", Self::Tabby),
		];
		match vars.into_iter().find(|v| env_exists(v.0)) {
			Some(var) => return var.1,
			None => warn!("[Adapter] No special environment variables detected"),
		}

		let (term, program) = Self::via_env();
		match program.as_str() {
			"iTerm.app" => return Self::Iterm2,
			"WezTerm" => return Self::WezTerm,
			"ghostty" => return Self::Ghostty,
			"rio" => return Self::Rio,
			"BlackBox" => return Self::BlackBox,
			"vscode" => return Self::VSCode,
			"Tabby" => return Self::Tabby,
			"Hyper" => return Self::Hyper,
			"mintty" => return Self::Mintty,
			"Apple_Terminal" => return Self::Apple,
			_ => warn!("[Adapter] Unknown TERM_PROGRAM: {program}"),
		}
		match term.as_str() {
			"xterm-kitty" => return Self::Kitty,
			"foot" => return Self::Foot,
			"foot-extra" => return Self::Foot,
			"xterm-ghostty" => return Self::Ghostty,
			"rio" => return Self::Rio,
			"rxvt-unicode-256color" => return Self::Urxvt,
			_ => warn!("[Adapter] Unknown TERM: {term}"),
		}

		Self::via_csi().unwrap_or(Self::Unknown(vec![]))
	}

	pub fn via_env() -> (String, String) {
		let (term, program) = Mux::term_program();
		(
			term.unwrap_or(std::env::var("TERM").unwrap_or_default()),
			program.unwrap_or(std::env::var("TERM_PROGRAM").unwrap_or_default()),
		)
	}

	pub fn via_csi() -> Result<Self> {
		defer! { disable_raw_mode().ok(); }
		enable_raw_mode()?;

		execute!(
			LineWriter::new(stderr()),
			SavePosition,
			Print(Mux::csi("\x1b[>q\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c")),
			RestorePosition
		)?;

		let resp = futures::executor::block_on(Self::read_until_da1());
		let names = [
			("kitty", Self::Kitty),
			("Konsole", Self::Konsole),
			("iTerm2", Self::Iterm2),
			("WezTerm", Self::WezTerm),
			("foot", Self::Foot),
			("ghostty", Self::Ghostty),
		];

		for (name, emulator) in names.iter() {
			if resp.contains(name) {
				return Ok(emulator.clone());
			}
		}

		let mut adapters = Vec::with_capacity(2);
		if resp.contains("\x1b_Gi=31;OK") {
			adapters.push(Adapter::KgpOld);
		}
		if ["?4;", "?4c", ";4;", ";4c"].iter().any(|s| resp.contains(s)) {
			adapters.push(Adapter::Sixel);
		}

		Ok(Self::Unknown(adapters))
	}

	pub fn move_lock<F, T>((x, y): (u16, u16), cb: F) -> Result<T>
	where
		F: FnOnce(&mut std::io::BufWriter<std::io::StderrLock>) -> Result<T>,
	{
		use std::{io::Write, thread, time::Duration};

		use crossterm::{cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show}, queue};

		let mut buf = std::io::BufWriter::new(stderr().lock());

		// I really don't want to add this,
		// But tmux and ConPTY sometimes cause the cursor position to get out of sync.
		if *TMUX || cfg!(windows) {
			execute!(buf, SavePosition, MoveTo(x, y), Show)?;
			execute!(buf, MoveTo(x, y), Show)?;
			execute!(buf, MoveTo(x, y), Show)?;
			thread::sleep(Duration::from_millis(1));
		} else {
			queue!(buf, SavePosition, MoveTo(x, y))?;
		}

		let result = cb(&mut buf);
		if *TMUX || cfg!(windows) {
			queue!(buf, Hide, RestorePosition)?;
		} else {
			queue!(buf, RestorePosition)?;
		}

		buf.flush()?;
		result
	}

	pub async fn read_until_da1() -> String {
		let mut buf: Vec<u8> = Vec::with_capacity(200);
		let read = async {
			let mut stdin = BufReader::new(tokio::io::stdin());
			loop {
				let mut c = [0; 1];
				if stdin.read(&mut c).await? == 0 {
					bail!("unexpected EOF");
				}
				buf.push(c[0]);
				if c[0] != b'c' || !buf.contains(&0x1b) {
					continue;
				}
				if buf.rsplitn(2, |&b| b == 0x1b).next().is_some_and(|s| s.starts_with(b"[?")) {
					break;
				}
			}
			Ok(())
		};

		match timeout(Duration::from_secs(10), read).await {
			Err(e) => error!("read_until_da1 timed out: {buf:?}, error: {e:?}"),
			Ok(Err(e)) => error!("read_until_da1 failed: {buf:?}, error: {e:?}"),
			Ok(Ok(())) => {}
		}
		String::from_utf8_lossy(&buf).into_owned()
	}
}
