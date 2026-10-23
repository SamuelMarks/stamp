//! Boot command macro parser and keyboard layout translation.

use std::time::Duration;

/// Keypress or delay action to execute during the boot typing phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootAction {
    /// Standard keypress with X11/RFB keysym.
    Key(u32),
    /// Explicit key down event with keysym.
    KeyDown(u32),
    /// Explicit key up event with keysym.
    KeyUp(u32),
    /// Pause execution for a given duration.
    Wait(Duration),
    /// Raw PS/2 scancode for `VirtualBox` / hypervisor direct input.
    Scancode(u8),
}

/// Parser for Packer boot command macro sequences.
#[derive(Debug, Clone, Copy, Default)]
pub struct BootCommandParser;

impl BootCommandParser {
    /// Parse a sequence of boot command string tokens into executable `BootAction` steps.
    #[must_use]
    pub fn parse(
        tokens: &[String],
        http_ip: Option<&str>,
        http_port: Option<u16>,
        boot_key_interval: Option<Duration>,
    ) -> Vec<BootAction> {
        Self::parse_with_intervals(tokens, http_ip, http_port, boot_key_interval, None)
    }

    /// Parse boot commands with both per-key interval and keygroup interval delays.
    #[must_use]
    pub fn parse_with_intervals(
        tokens: &[String],
        http_ip: Option<&str>,
        http_port: Option<u16>,
        boot_key_interval: Option<Duration>,
        boot_keygroup_interval: Option<Duration>,
    ) -> Vec<BootAction> {
        let mut actions = Vec::new();
        let default_interval = boot_key_interval.unwrap_or(Duration::from_millis(50));

        let ip_sub = http_ip.unwrap_or("127.0.0.1");
        let port_sub = http_port.unwrap_or(8080).to_string();

        for (i, raw_token) in tokens.iter().enumerate() {
            if i > 0
                && let Some(group_interval) = boot_keygroup_interval
            {
                actions.push(BootAction::Wait(group_interval));
            }

            let token = raw_token
                .replace("{{ .HTTPIP }}", ip_sub)
                .replace("{{ .HTTPPort }}", &port_sub)
                .replace("{{.HTTPIP}}", ip_sub)
                .replace("{{.HTTPPort}}", &port_sub);

            let mut chars = token.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '<' {
                    let mut tag = String::new();
                    for inner in chars.by_ref() {
                        if inner == '>' {
                            break;
                        }
                        tag.push(inner);
                    }
                    Self::parse_tag(&tag, default_interval, &mut actions);
                } else {
                    actions.push(BootAction::Key(ch as u32));
                    actions.push(BootAction::Wait(default_interval));
                }
            }
        }

        Self::ensure_modifiers_released(&mut actions);
        actions
    }

    /// Parse a single `<tag>` token into `BootAction`s.
    fn parse_tag(tag: &str, default_interval: Duration, actions: &mut Vec<BootAction>) {
        let tag_lower = tag.to_lowercase();
        if tag_lower == "wait" {
            actions.push(BootAction::Wait(Duration::from_secs(1)));
            return;
        }
        if let Some(rest) = tag_lower.strip_prefix("wait") {
            let dur = Self::parse_wait_duration(rest);
            actions.push(BootAction::Wait(dur));
            return;
        }
        let keysym = match tag_lower.as_str() {
            "enter" | "return" => Some(0xFF0D),
            "tab" => Some(0xFF09),
            "esc" | "escape" => Some(0xFF1B),
            "bs" | "backspace" => Some(0xFF08),
            "space" | "spacebar" => Some(0x0020),
            "up" => Some(0xFF52),
            "down" => Some(0xFF54),
            "left" => Some(0xFF51),
            "right" => Some(0xFF53),
            "pageup" | "page_up" => Some(0xFF55),
            "pagedown" | "page_down" => Some(0xFF56),
            "home" => Some(0xFF50),
            "end" => Some(0xFF57),
            "insert" => Some(0xFF63),
            "delete" | "del" => Some(0xFFFF),
            _ => None,
        };
        if let Some(sym) = keysym {
            actions.push(BootAction::Key(sym));
            actions.push(BootAction::Wait(default_interval));
            return;
        }
        if let Some(f_idx) = tag_lower.strip_prefix('f')
            && let Ok(num) = f_idx.parse::<u32>()
            && (1..=12).contains(&num)
        {
            actions.push(BootAction::Key(0xFFBD + num));
            actions.push(BootAction::Wait(default_interval));
            return;
        }
        let modifier = match tag_lower.as_str() {
            "leftshifton" => Some(BootAction::KeyDown(0xFFE1)),
            "leftshiftoff" => Some(BootAction::KeyUp(0xFFE1)),
            "rightshifton" => Some(BootAction::KeyDown(0xFFE2)),
            "rightshiftoff" => Some(BootAction::KeyUp(0xFFE2)),
            "leftctrlon" => Some(BootAction::KeyDown(0xFFE3)),
            "leftctrloff" => Some(BootAction::KeyUp(0xFFE3)),
            "rightctrlon" => Some(BootAction::KeyDown(0xFFE4)),
            "rightctrloff" => Some(BootAction::KeyUp(0xFFE4)),
            "leftalton" => Some(BootAction::KeyDown(0xFFE9)),
            "leftaltoff" => Some(BootAction::KeyUp(0xFFE9)),
            "rightalton" => Some(BootAction::KeyDown(0xFFEA)),
            "rightaltoff" => Some(BootAction::KeyUp(0xFFEA)),
            "superon" | "leftsuperon" => Some(BootAction::KeyDown(0xFFEB)),
            "superoff" | "leftsuperoff" => Some(BootAction::KeyUp(0xFFEB)),
            "rightsuperon" => Some(BootAction::KeyDown(0xFFEC)),
            "rightsuperoff" => Some(BootAction::KeyUp(0xFFEC)),
            _ => None,
        };
        if let Some(mod_action) = modifier {
            actions.push(mod_action);
        } else {
            for tc in tag.chars() {
                actions.push(BootAction::Key(tc as u32));
                actions.push(BootAction::Wait(default_interval));
            }
        }
    }

    /// Ensures that any modifier keys (Shift, Ctrl, Alt) remaining in a pressed state
    /// are released with trailing `KeyUp` actions to avoid leaving the guest in a stuck state.
    pub fn ensure_modifiers_released(actions: &mut Vec<BootAction>) {
        let mut pressed_modifiers = std::collections::BTreeSet::new();
        for action in actions.iter() {
            match action {
                BootAction::KeyDown(k) => {
                    pressed_modifiers.insert(*k);
                }
                BootAction::KeyUp(k) => {
                    pressed_modifiers.remove(k);
                }
                _ => {}
            }
        }
        for modifier in pressed_modifiers {
            actions.push(BootAction::KeyUp(modifier));
        }
    }

    /// Parse duration string such as `5s`, `500ms`, `2m`, or plain integer seconds.
    fn parse_wait_duration(s: &str) -> Duration {
        let trimmed = s.trim();
        if let Some(ms_str) = trimmed.strip_suffix("ms")
            && let Ok(ms) = ms_str.parse::<u64>()
        {
            return Duration::from_millis(ms);
        }
        if let Some(s_str) = trimmed.strip_suffix('s')
            && let Ok(secs) = s_str.parse::<u64>()
        {
            return Duration::from_secs(secs);
        }
        if let Some(m_str) = trimmed.strip_suffix('m')
            && let Ok(mins) = m_str.parse::<u64>()
        {
            return Duration::from_secs(mins * 60);
        }
        if let Ok(secs) = trimmed.parse::<u64>() {
            return Duration::from_secs(secs);
        }
        Duration::from_secs(1)
    }
}

/// Localized keyboard layouts for boot command scancode / keysym translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyboardLayout {
    /// Standard US QWERTY keyboard layout.
    #[default]
    Us,
    /// United Kingdom (UK) layout.
    Uk,
    /// German (DE) QWERTZ layout.
    De,
    /// French (FR) AZERTY layout.
    Fr,
}

impl KeyboardLayout {
    /// Translates an ASCII character into the target layout keysym.
    #[must_use]
    pub fn translate_char(&self, ch: char) -> u32 {
        match self {
            Self::Us | Self::Uk => ch as u32,
            Self::De => match ch {
                'y' => 'z' as u32,
                'z' => 'y' as u32,
                'Y' => 'Z' as u32,
                'Z' => 'Y' as u32,
                other => other as u32,
            },
            Self::Fr => match ch {
                'a' => 'q' as u32,
                'q' => 'a' as u32,
                'w' => 'z' as u32,
                'z' => 'w' as u32,
                'A' => 'Q' as u32,
                'Q' => 'A' as u32,
                'W' => 'Z' as u32,
                'Z' => 'W' as u32,
                other => other as u32,
            },
        }
    }
}
