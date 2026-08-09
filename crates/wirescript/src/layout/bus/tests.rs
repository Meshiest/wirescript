    use super::*;
    use crate::ir::port_registry::WirePort;

    fn demand(rows: &[usize], consumers: usize, order: usize) -> BusDemand {
        BusDemand {
            source: PortRef {
                node_id: crate::ir::NodeId::fresh(),
                port: WirePort::RerOutput,
            },
            rows: rows.to_vec(),
            consumers,
            source_order: order,
        }
    }

    #[test]
    fn longest_span_takes_the_leftmost_lane() {
        // Two overlapping demands: the wider span wins lane 0.
        let d = vec![demand(&[5, 6], 2, 0), demand(&[0, 9], 2, 1)];
        let lanes = allocate_lanes(&d);
        assert_eq!(lanes[1], 0, "the 0..9 span holds the leftmost lane");
        assert_eq!(lanes[0], 1, "the shorter overlapping span goes right");
    }

    #[test]
    fn a_lane_is_reused_once_its_range_ends() {
        // Disjoint ranges share one lane.
        let d = vec![demand(&[0, 2], 2, 0), demand(&[5, 7], 2, 1)];
        let lanes = allocate_lanes(&d);
        assert_eq!(lanes[0], lanes[1], "disjoint ranges reuse the same lane");
    }

    #[test]
    fn touching_ranges_do_not_share_a_lane() {
        // Ending and starting on the SAME row must not share — both need a
        // rerouter on that row.
        let d = vec![demand(&[0, 4], 2, 0), demand(&[4, 8], 2, 1)];
        let lanes = allocate_lanes(&d);
        assert_ne!(lanes[0], lanes[1], "ranges sharing row 4 need separate lanes");
    }

    #[test]
    fn consumer_count_breaks_span_ties() {
        let d = vec![demand(&[0, 5], 2, 0), demand(&[0, 5], 9, 1)];
        let lanes = allocate_lanes(&d);
        assert_eq!(lanes[1], 0, "the hotter value takes the leftmost lane");
    }

    #[test]
    fn allocation_is_deterministic() {
        let d = vec![
            demand(&[0, 9], 3, 0),
            demand(&[1, 2], 2, 1),
            demand(&[4, 8], 5, 2),
            demand(&[3, 3], 1, 3),
        ];
        assert_eq!(allocate_lanes(&d), allocate_lanes(&d));
    }

    #[test]
    fn single_row_demands_pack_into_few_lanes() {
        // Four one-row demands on distinct rows all fit in lane 0.
        let d = vec![
            demand(&[0], 1, 0),
            demand(&[1], 1, 1),
            demand(&[2], 1, 2),
            demand(&[3], 1, 3),
        ];
        assert_eq!(allocate_lanes(&d), vec![0, 0, 0, 0]);
    }
