pub fn remove_underscores(s: &str) -> String {
    s.chars().filter(|x| x != &'_').collect()
}
