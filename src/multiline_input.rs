use anyhow::Result;

/// 複数行テキスト入力のバッファ状態を管理する。
/// ターミナルの raw モード処理とは分離されており、単体テスト可能。
pub struct MultilineEditor {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize, // 文字単位（バイトではなく）
}

impl MultilineEditor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    /// カーソル位置に文字を挿入する。
    pub fn insert_char(&mut self, ch: char) {
        let line = &mut self.lines[self.cursor_row];
        let byte_pos = line
            .char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        line.insert(byte_pos, ch);
        self.cursor_col += 1;
    }

    /// カーソル位置で改行を挿入する（Shift+Enter）。
    pub fn insert_newline(&mut self) {
        let line = &self.lines[self.cursor_row];
        let byte_pos = line
            .char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        let remainder = line[byte_pos..].to_string();
        self.lines[self.cursor_row].truncate(byte_pos);
        self.lines.insert(self.cursor_row + 1, remainder);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    /// カーソル直前の文字を削除する（Backspace）。
    pub fn delete_char(&mut self) {
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_row];
            let byte_pos = line
                .char_indices()
                .nth(self.cursor_col - 1)
                .map(|(i, _)| i)
                .unwrap();
            line.remove(byte_pos);
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            let current_line = self.lines.remove(self.cursor_row);
            let prev_len = self.lines[self.cursor_row - 1].chars().count();
            self.lines[self.cursor_row - 1].push_str(&current_line);
            self.cursor_row -= 1;
            self.cursor_col = prev_len;
        }
    }

    /// カーソルを左に移動する。
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].chars().count();
        }
    }

    /// カーソルを右に移動する。
    pub fn move_right(&mut self) {
        let line_len = self.lines[self.cursor_row].chars().count();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row < self.lines.len() - 1 {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// カーソルを上の行に移動する。
    /// col=0 の場合は前の行末へ、それ以外は前の行の同列（または行末）へ移動する。
    pub fn move_up(&mut self) {
        if self.cursor_row == 0 {
            return;
        }
        let prev_len = self.lines[self.cursor_row - 1].chars().count();
        self.cursor_row -= 1;
        if self.cursor_col == 0 {
            self.cursor_col = prev_len;
        } else {
            self.cursor_col = self.cursor_col.min(prev_len);
        }
    }

    /// カーソルを下の行に移動する。
    pub fn move_down(&mut self) {
        if self.cursor_row >= self.lines.len() - 1 {
            return;
        }
        let next_len = self.lines[self.cursor_row + 1].chars().count();
        self.cursor_row += 1;
        self.cursor_col = self.cursor_col.min(next_len);
    }

    /// バッファ全体の文字列を返す。
    pub fn to_string(&self) -> String {
        self.lines.join("\n")
    }

    /// バッファが空かどうかを返す。
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// 行数を返す。
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// 現在のカーソル行番号を返す。
    pub fn cursor_row(&self) -> usize {
        self.cursor_row
    }

    /// 現在のカーソル列番号（文字単位）を返す。
    pub fn cursor_col(&self) -> usize {
        self.cursor_col
    }

    /// 現在のカーソル行のテキストを返す。
    pub fn current_line(&self) -> &str {
        &self.lines[self.cursor_row]
    }
}

/// ターミナルで複数行テキスト入力を読み取る。
/// - `Enter` で入力確定
/// - `Shift+Enter` で改行挿入
/// - `Ctrl+C` / `Esc` でキャンセル（`None` を返す）
pub fn read_multiline(prompt: &str) -> Result<Option<String>> {
    use crossterm::terminal;

    eprintln!("{} (Shift+Enter で改行、Enter で送信)", prompt);

    terminal::enable_raw_mode()?;
    let result = read_multiline_raw();
    terminal::disable_raw_mode()?;

    result
}

fn read_multiline_raw() -> Result<Option<String>> {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};

    let mut editor = MultilineEditor::new();

    loop {
        let event = event::read()?;
        if let Event::Key(key_event) = event {
            match (key_event.code, key_event.modifiers) {
                // Ctrl+C または Esc でキャンセル
                (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                    return Ok(None);
                }
                // Shift+Enter で改行
                (KeyCode::Enter, KeyModifiers::SHIFT) => {
                    editor.insert_newline();
                }
                // Enter で確定
                (KeyCode::Enter, _) => {
                    let text = editor.to_string();
                    return Ok(Some(text));
                }
                // Backspace で文字削除
                (KeyCode::Backspace, _) => {
                    editor.delete_char();
                }
                // 矢印キー
                (KeyCode::Left, _) => editor.move_left(),
                (KeyCode::Right, _) => editor.move_right(),
                (KeyCode::Up, _) => editor.move_up(),
                (KeyCode::Down, _) => editor.move_down(),
                // 通常文字の入力
                (KeyCode::Char(ch), _) => {
                    editor.insert_char(ch);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- MultilineEditor 初期状態のテスト ---

    #[test]
    fn test_new_editor_is_empty() {
        let editor = MultilineEditor::new();
        assert!(editor.is_empty());
    }

    #[test]
    fn test_new_editor_has_one_line() {
        let editor = MultilineEditor::new();
        assert_eq!(editor.line_count(), 1);
    }

    #[test]
    fn test_new_editor_cursor_at_origin() {
        let editor = MultilineEditor::new();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 0);
    }

    #[test]
    fn test_new_editor_to_string_is_empty_string() {
        let editor = MultilineEditor::new();
        assert_eq!(editor.to_string(), "");
    }

    // --- insert_char のテスト ---

    #[test]
    fn test_insert_single_char() {
        let mut editor = MultilineEditor::new();
        editor.insert_char('a');
        assert_eq!(editor.to_string(), "a");
        assert!(!editor.is_empty());
    }

    #[test]
    fn test_insert_multiple_chars() {
        let mut editor = MultilineEditor::new();
        for ch in "hello".chars() {
            editor.insert_char(ch);
        }
        assert_eq!(editor.to_string(), "hello");
    }

    #[test]
    fn test_insert_char_moves_cursor_right() {
        let mut editor = MultilineEditor::new();
        editor.insert_char('a');
        assert_eq!(editor.cursor_col(), 1);
    }

    #[test]
    fn test_insert_multibyte_char() {
        let mut editor = MultilineEditor::new();
        editor.insert_char('あ');
        editor.insert_char('い');
        assert_eq!(editor.to_string(), "あい");
        // 文字単位でカーソルが 2 進む
        assert_eq!(editor.cursor_col(), 2);
    }

    // --- insert_newline のテスト ---

    #[test]
    fn test_insert_newline_increases_line_count() {
        let mut editor = MultilineEditor::new();
        editor.insert_newline();
        assert_eq!(editor.line_count(), 2);
    }

    #[test]
    fn test_insert_newline_moves_cursor_to_next_line() {
        let mut editor = MultilineEditor::new();
        editor.insert_newline();
        assert_eq!(editor.cursor_row(), 1);
        assert_eq!(editor.cursor_col(), 0);
    }

    #[test]
    fn test_insert_newline_splits_line() {
        let mut editor = MultilineEditor::new();
        for ch in "hello".chars() {
            editor.insert_char(ch);
        }
        // カーソルを中間に移動
        editor.move_left();
        editor.move_left();
        // "hel" の後ろで改行
        editor.insert_newline();
        assert_eq!(editor.line_count(), 2);
        assert_eq!(editor.to_string(), "hel\nlo");
    }

    #[test]
    fn test_multiline_to_string() {
        let mut editor = MultilineEditor::new();
        for ch in "line1".chars() {
            editor.insert_char(ch);
        }
        editor.insert_newline();
        for ch in "line2".chars() {
            editor.insert_char(ch);
        }
        assert_eq!(editor.to_string(), "line1\nline2");
    }

    // --- delete_char のテスト ---

    #[test]
    fn test_delete_char_removes_last_char() {
        let mut editor = MultilineEditor::new();
        for ch in "hello".chars() {
            editor.insert_char(ch);
        }
        editor.delete_char();
        assert_eq!(editor.to_string(), "hell");
    }

    #[test]
    fn test_delete_char_moves_cursor_left() {
        let mut editor = MultilineEditor::new();
        editor.insert_char('a');
        editor.delete_char();
        assert_eq!(editor.cursor_col(), 0);
    }

    #[test]
    fn test_delete_char_at_start_of_line_merges_with_previous() {
        let mut editor = MultilineEditor::new();
        for ch in "line1".chars() {
            editor.insert_char(ch);
        }
        editor.insert_newline();
        for ch in "line2".chars() {
            editor.insert_char(ch);
        }
        // "line2" の先頭へ移動
        editor.move_left();
        editor.move_left();
        editor.move_left();
        editor.move_left();
        editor.move_left();
        // 行頭で Backspace → 前の行と結合
        editor.delete_char();
        assert_eq!(editor.line_count(), 1);
        assert_eq!(editor.to_string(), "line1line2");
    }

    #[test]
    fn test_delete_char_on_empty_editor_does_nothing() {
        let mut editor = MultilineEditor::new();
        editor.delete_char(); // パニックしないことを確認
        assert_eq!(editor.to_string(), "");
    }

    // --- move_left / move_right のテスト ---

    #[test]
    fn test_move_left_decrements_cursor_col() {
        let mut editor = MultilineEditor::new();
        for ch in "ab".chars() {
            editor.insert_char(ch);
        }
        editor.move_left();
        assert_eq!(editor.cursor_col(), 1);
    }

    #[test]
    fn test_move_left_at_start_of_line_goes_to_previous_line_end() {
        let mut editor = MultilineEditor::new();
        for ch in "line1".chars() {
            editor.insert_char(ch);
        }
        editor.insert_newline();
        // 新しい行の先頭
        assert_eq!(editor.cursor_row(), 1);
        assert_eq!(editor.cursor_col(), 0);
        editor.move_left();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 5); // "line1" の末尾
    }

    #[test]
    fn test_move_right_increments_cursor_col() {
        let mut editor = MultilineEditor::new();
        for ch in "ab".chars() {
            editor.insert_char(ch);
        }
        editor.move_left();
        editor.move_left();
        editor.move_right();
        assert_eq!(editor.cursor_col(), 1);
    }

    #[test]
    fn test_move_right_at_end_of_line_goes_to_next_line_start() {
        let mut editor = MultilineEditor::new();
        for ch in "line1".chars() {
            editor.insert_char(ch);
        }
        editor.insert_newline();
        for ch in "line2".chars() {
            editor.insert_char(ch);
        }
        // 行頭へ
        editor.move_left();
        editor.move_left();
        editor.move_left();
        editor.move_left();
        editor.move_left();
        // 行末（1行目）へ
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        // "line1" の末尾
        assert_eq!(editor.cursor_col(), 5);
        editor.move_right();
        assert_eq!(editor.cursor_row(), 1);
        assert_eq!(editor.cursor_col(), 0);
    }

    // --- move_up / move_down のテスト ---

    #[test]
    fn test_move_up_goes_to_previous_row() {
        let mut editor = MultilineEditor::new();
        editor.insert_newline();
        assert_eq!(editor.cursor_row(), 1);
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
    }

    #[test]
    fn test_move_up_at_first_row_does_nothing() {
        let mut editor = MultilineEditor::new();
        editor.move_up(); // パニックしないことを確認
        assert_eq!(editor.cursor_row(), 0);
    }

    #[test]
    fn test_move_down_goes_to_next_row() {
        let mut editor = MultilineEditor::new();
        editor.insert_newline();
        editor.move_up();
        editor.move_down();
        assert_eq!(editor.cursor_row(), 1);
    }

    #[test]
    fn test_move_down_at_last_row_does_nothing() {
        let mut editor = MultilineEditor::new();
        editor.move_down(); // パニックしないことを確認
        assert_eq!(editor.cursor_row(), 0);
    }

    #[test]
    fn test_move_up_clamps_cursor_col_to_line_length() {
        let mut editor = MultilineEditor::new();
        // 1行目: "hi"（2文字）
        editor.insert_char('h');
        editor.insert_char('i');
        editor.insert_newline();
        // 2行目: "hello world"（11文字）
        for ch in "hello world".chars() {
            editor.insert_char(ch);
        }
        assert_eq!(editor.cursor_col(), 11);
        // 1行目に戻るとカーソルが "hi" の末尾にクランプされる
        editor.move_up();
        assert_eq!(editor.cursor_row(), 0);
        assert_eq!(editor.cursor_col(), 2);
    }

    // --- current_line のテスト ---

    #[test]
    fn test_current_line_returns_correct_line() {
        let mut editor = MultilineEditor::new();
        for ch in "first".chars() {
            editor.insert_char(ch);
        }
        editor.insert_newline();
        for ch in "second".chars() {
            editor.insert_char(ch);
        }
        assert_eq!(editor.current_line(), "second");
        editor.move_up();
        assert_eq!(editor.current_line(), "first");
    }

    // --- カーソル位置での文字挿入テスト ---

    #[test]
    fn test_insert_char_at_middle_of_line() {
        let mut editor = MultilineEditor::new();
        for ch in "hllo".chars() {
            editor.insert_char(ch);
        }
        // 'l' の前にカーソルを移動（"h" の後ろ）
        editor.move_left();
        editor.move_left();
        editor.move_left();
        editor.insert_char('e');
        assert_eq!(editor.to_string(), "hello");
    }
}
