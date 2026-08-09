    use super::*;

    #[test]
    fn roundtrip() {
        let s = intern("hello");
        assert_eq!(resolve(s), "hello");
    }

    #[test]
    fn dedup() {
        let a = intern("world");
        let b = intern("world");
        assert_eq!(a, b);
    }

    #[test]
    fn static_intern() {
        let s = intern_static("static_str");
        assert_eq!(resolve(s), "static_str");
    }
