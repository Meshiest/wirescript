pub fn remove_underscores(s: &str) -> String {
    s.chars().filter(|x| x != &'_').collect()
}

pub fn strs_to_strings(s: Vec<&str>) -> Vec<String> {
    s.into_iter().map(|s| s.to_string()).collect()
}

pub fn fmt_iter(
    f: &mut std::fmt::Formatter<'_>,
    mut iter: impl Iterator<Item = impl std::fmt::Display>,
    sep: &str,
) -> std::fmt::Result {
    let Some(first) = iter.next() else {
        return Ok(());
    };

    first.fmt(f)?;
    for item in iter {
        f.write_str(sep)?;
        item.fmt(f)?;
    }

    Ok(())
}
