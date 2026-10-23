//! The text screen the original leaves behind when it quits (fl2.c): the
//! shareware copies ENDTEXT.BIN, an 80x25 VGA text page, to video memory;
//! without it (the registered data) DrawBar puts the title in a bar on the
//! top line. Platforms print it in a terminal or on the page.

use crate::data::Data;

const COLS: usize = 80;
const ROWS: usize = 25;

/// The standard VGA text mode colours.
pub const VGA_RGB: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], [0x00, 0x00, 0xaa], [0x00, 0xaa, 0x00], [0x00, 0xaa, 0xaa],
    [0xaa, 0x00, 0x00], [0xaa, 0x00, 0xaa], [0xaa, 0x55, 0x00], [0xaa, 0xaa, 0xaa],
    [0x55, 0x55, 0x55], [0x55, 0x55, 0xff], [0x55, 0xff, 0x55], [0x55, 0xff, 0xff],
    [0xff, 0x55, 0x55], [0xff, 0x55, 0xff], [0xff, 0xff, 0x55], [0xff, 0xff, 0xff],
];

const CP437_LO: &str = " ☺☻♥♦♣♠•◘○◙♂♀♪♫☼►◄↕‼¶§▬↨↑↓→←∟↔▲▼";
const CP437_HI: &str = concat!(
    "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒ",
    "áíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐",
    "└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀",
    "αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}",
);

/// A code page 437 character as Unicode.
pub fn cp437(c: u8) -> char {
    match c {
        0..0x20 => CP437_LO.chars().nth(c as usize).unwrap(),
        0x7f => '⌂',
        0x80.. => CP437_HI.chars().nth(c as usize - 0x80).unwrap(),
        _ => c as char,
    }
}

/// An 80x25 text page: (character, attribute) cells. Attributes are VGA's:
/// foreground in the low nibble, background in bits 4-6 (bit 7 blinks).
pub struct TextScreen {
    cells: Vec<(u8, u8)>,
    /// Where DOS's prompt came back: the rows above it are the screen.
    cursor_row: usize,
}

impl TextScreen {
    /// What the original showed on quitting with this data.
    pub fn exit_screen(data: &Data) -> TextScreen {
        if let Some(b) = data.get("endtext.bin").filter(|b| b.len() == COLS * ROWS * 2) {
            return TextScreen { cells: b.chunks(2).map(|c| (c[0], c[1])).collect(), cursor_row: 22 };
        }
        // DrawBar: TXS_DrawString(0, 80, 0, ..., 0x2E, TXS_STCENTER).
        let title = b"SPEED HASTE v1.00";
        let x = (COLS + 1 - title.len()) / 2;
        let mut cells = vec![(b' ', 0x07); COLS * ROWS];
        for (i, c) in cells[..COLS].iter_mut().enumerate() {
            *c = (i.checked_sub(x).and_then(|j| title.get(j)).copied().unwrap_or(b' '), 0x2e);
        }
        TextScreen { cells, cursor_row: 1 }
    }

    /// The rows above the cursor, without trailing blank black rows, as
    /// runs of (text, attribute). Text in its own background colour (the
    /// shareware's file hides leftovers that way) becomes spaces.
    pub fn rows(&self) -> Vec<Vec<(String, u8)>> {
        let blank = |&(c, a): &(u8, u8)| (c == b' ' || c == 0) && a & 0x70 == 0;
        let mut rows: Vec<&[(u8, u8)]> = self.cells.chunks(COLS).take(self.cursor_row).collect();
        while rows.last().is_some_and(|r| r.iter().all(blank)) {
            rows.pop();
        }
        rows.iter()
            .map(|row| {
                let mut runs: Vec<(String, u8)> = Vec::new();
                for &(c, a) in *row {
                    let a = a & 0x7f;
                    let ch = if a & 15 == a >> 4 { ' ' } else { cp437(c) };
                    match runs.last_mut() {
                        Some((s, last)) if *last == a => s.push(ch),
                        _ => runs.push((ch.to_string(), a)),
                    }
                }
                runs
            })
            .collect()
    }

    /// For a terminal: 24-bit colour escapes, or plain text without.
    pub fn to_terminal(&self, colour: bool) -> String {
        let mut out = String::new();
        for row in self.rows() {
            if colour {
                for (text, a) in row {
                    let ([fr, fg, fb], [br, bg, bb]) = (VGA_RGB[a as usize & 15], VGA_RGB[a as usize >> 4]);
                    out += &format!("\x1b[38;2;{fr};{fg};{fb};48;2;{br};{bg};{bb}m{text}");
                }
                out += "\x1b[0m\n";
            } else {
                let line: String = row.into_iter().map(|(t, _)| t).collect();
                out += line.trim_end();
                out.push('\n');
            }
        }
        out
    }
}
