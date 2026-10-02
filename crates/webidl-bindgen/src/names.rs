pub(crate) fn snake_case(name: &str) -> String {
    let mut snake = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (index, char) in chars.iter().enumerate() {
        if char.is_ascii_uppercase() {
            let before_lower = index > 0
                && (chars[index - 1].is_ascii_lowercase() || chars[index - 1].is_ascii_digit());
            let after_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if index > 0 && (before_lower || after_lower) {
                snake.push('_');
            }
            snake.push(char.to_ascii_lowercase());
        } else {
            snake.push(*char);
        }
    }
    snake
}
