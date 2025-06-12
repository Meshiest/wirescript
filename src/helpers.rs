pub fn remove_underscores(s: &str) -> String {
    s.chars().filter(|x| x != &'_').collect()
}

pub fn strs_to_strings(s: Vec<&str>) -> Vec<String> {
    s.into_iter().map(|s| s.to_string()).collect()
}
