/// Conservative tokenizer-free estimate calibrated for JSON-heavy Jev state.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    let mut token_tenths = 0_usize;
    let mut letters = 0_usize;
    let mut digits = 0_usize;
    for character in text.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_alphabetic() {
            flush_digits(&mut token_tenths, &mut digits);
            letters += 1;
        } else if character.is_ascii_digit() {
            flush_letters(&mut token_tenths, &mut letters);
            digits += 1;
        } else {
            flush_letters(&mut token_tenths, &mut letters);
            flush_digits(&mut token_tenths, &mut digits);
            if !character.is_whitespace() {
                token_tenths = token_tenths.saturating_add(9);
            }
        }
    }
    token_tenths.saturating_add(9) / 10
}

fn flush_letters(tokens: &mut usize, letters: &mut usize) {
    if *letters > 0 {
        *tokens = tokens.saturating_add(10 * (1 + letters.saturating_sub(1) / 6));
        *letters = 0;
    }
}

fn flush_digits(tokens: &mut usize, digits: &mut usize) {
    if *digits > 0 {
        *tokens = tokens.saturating_add(digits.saturating_mul(5));
        *digits = 0;
    }
}
