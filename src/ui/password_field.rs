use egui::TextBuffer;

const MASK: char = '\u{2022}';

pub struct MaskedBuffer<'a> {
    secret: &'a mut String,
    display: String,
}

impl<'a> MaskedBuffer<'a> {
    pub fn new(secret: &'a mut String) -> Self {
        let display = MASK.to_string().repeat(secret.chars().count());
        Self { secret, display }
    }
}

impl<'a> TextBuffer for MaskedBuffer<'a> {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        &self.display
    }

    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        let inserted = text.chars().count();

        let secret_byte = byte_index_for_char(self.secret, char_index);
        self.secret.insert_str(secret_byte, text);

        let display_byte = byte_index_for_char(&self.display, char_index);
        let mask_str: String = std::iter::repeat(MASK).take(inserted).collect();
        self.display.insert_str(display_byte, &mask_str);

        inserted
    }

    fn delete_char_range(&mut self, char_range: std::ops::Range<usize>) {
        let s_start = byte_index_for_char(self.secret, char_range.start);
        let s_end = byte_index_for_char(self.secret, char_range.end);
        self.secret.replace_range(s_start..s_end, "");

        let d_start = byte_index_for_char(&self.display, char_range.start);
        let d_end = byte_index_for_char(&self.display, char_range.end);
        self.display.replace_range(d_start..d_end, "");
    }

    fn clear(&mut self) {
        self.secret.clear();
        self.display.clear();
    }

    fn replace_with(&mut self, text: &str) {
        self.secret.clear();
        self.secret.push_str(text);
        self.display.clear();
        for _ in text.chars() {
            self.display.push(MASK);
        }
    }

    fn take(&mut self) -> String {
        self.display.clear();
        std::mem::take(self.secret)
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<String>()
    }
}

fn byte_index_for_char(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}
