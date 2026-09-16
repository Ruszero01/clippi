//! Stable insertion relative to an existing item, including filtered views.

pub fn move_relative<T>(items: &mut Vec<T>, source: usize, target: usize, after: bool) -> bool {
    if source >= items.len() || target >= items.len() || source == target {
        return false;
    }
    let destination = target + usize::from(after) - usize::from(source < target);
    if source == destination {
        return false;
    }
    let item = items.remove(source);
    items.insert(destination, item);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_works_in_both_directions_and_at_edges() {
        let mut items = vec![1, 2, 3, 4];
        assert!(move_relative(&mut items, 0, 3, true));
        assert_eq!(items, [2, 3, 4, 1]);
        assert!(move_relative(&mut items, 3, 0, false));
        assert_eq!(items, [1, 2, 3, 4]);
        assert!(move_relative(&mut items, 0, 2, false));
        assert_eq!(items, [2, 1, 3, 4]);
        assert!(move_relative(&mut items, 3, 0, true));
        assert_eq!(items, [2, 4, 1, 3]);
    }

    #[test]
    fn invalid_and_unchanged_drops_are_noops() {
        let mut items = vec![1, 2, 3];
        for (source, target, after) in [
            (0, 0, true),
            (0, 1, false),
            (1, 0, true),
            (8, 1, false),
            (0, 8, true),
        ] {
            assert!(!move_relative(&mut items, source, target, after));
            assert_eq!(items, [1, 2, 3]);
        }
        assert!(!move_relative::<u8>(&mut vec![], 0, 0, false));
    }
}
