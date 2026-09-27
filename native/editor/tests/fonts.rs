use ad3_editor::{
    fonts::{FontBook, FontSpec},
    text_layout::Baseline,
};

#[test]
fn bundled_inter_baselines_match_canvas_metrics() {
    let mut book = FontBook::default();
    let spec = FontSpec {
        size: 32.0,
        ..Default::default()
    };
    for (baseline, offset) in [
        (Baseline::Alphabetic, 0.0),
        (Baseline::Top, 25.625),
        (Baseline::Middle, 9.625),
        (Baseline::Bottom, -6.375),
    ] {
        let result = book.shape("Hello", &spec, baseline, None).unwrap();
        assert!((result.metrics.width - 70.5625).abs() < 0.1);
        assert_eq!(result.metrics.ascent, 31.0 - offset);
        assert_eq!(result.metrics.descent, 8.0 + offset);
        assert!(result.path.is_some());
        let empty = book.shape("", &spec, baseline, None).unwrap();
        assert_eq!(empty.metrics.width, 0.0);
        assert_eq!(empty.metrics.ink_ascent, -offset);
        assert_eq!(empty.metrics.ink_descent, offset);
        assert!(empty.path.is_none());
    }
}

#[derive(serde::Deserialize, Debug)]
struct Case {
    size: f64,
    style: String,
    weight: f64,
    spacing: f64,
    baseline: String,
    text: String,
    metrics: ad3_editor::text_layout::Metrics,
}
#[test]
fn bundled_font_shaping_matches_canvas_reference_matrix() {
    #[derive(serde::Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/text-metrics.json")).unwrap();
    let mut book = FontBook::default();
    for case in fixture.cases {
        let baseline = match case.baseline.as_str() {
            "top" => Baseline::Top,
            "middle" => Baseline::Middle,
            "bottom" => Baseline::Bottom,
            _ => Baseline::Alphabetic,
        };
        let spec = FontSpec {
            size: case.size,
            weight: case.weight,
            spacing: case.spacing,
            style: if case.style == "italic" {
                fontdb::Style::Italic
            } else {
                fontdb::Style::Normal
            },
            ..Default::default()
        };
        let shaped = book.shape(&case.text, &spec, baseline, None).unwrap();
        assert!(
            (shaped.metrics.width - case.metrics.width).abs() <= 0.125,
            "{case:?}: {:?}",
            shaped.metrics
        );
        assert_eq!(shaped.metrics.ascent, case.metrics.ascent, "{case:?}");
        assert_eq!(shaped.metrics.descent, case.metrics.descent, "{case:?}");
        if case.text.trim().is_empty() {
            assert_eq!(
                shaped.metrics.ink_ascent, case.metrics.ink_ascent,
                "{case:?}"
            );
            assert_eq!(
                shaped.metrics.ink_descent, case.metrics.ink_descent,
                "{case:?}"
            );
        }
    }
}

#[test]
fn font_cache_cancellation_and_unsupported_sequences_are_explicit() {
    use std::sync::{Arc, atomic::AtomicBool};
    let mut book = FontBook::default();
    let spec = FontSpec::default();
    let first = book.shape("Hello", &spec, Baseline::Top, None).unwrap();
    let second = book.shape("Hello", &spec, Baseline::Top, None).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(
        book.shape("Hello", &spec, Baseline::Top, Some(&AtomicBool::new(true)))
            .is_err()
    );
    for text in ["\u{2764}\u{fe0f}", "a\u{200d}b"] {
        assert!(
            book.shape(text, &spec, Baseline::Top, None)
                .err()
                .unwrap()
                .contains("not ported")
        );
    }
    assert!(
        book.shape(&"a".repeat(100_001), &spec, Baseline::Top, None)
            .is_err()
    );
    let bad = FontSpec {
        size: f64::INFINITY,
        ..spec
    };
    assert!(book.shape("hello", &bad, Baseline::Top, None).is_err());
}

#[test]
fn text_layout_matches_original_tokenize_and_shape_functions() {
    use ad3_editor::text_layout::{Align, Piece, layout};
    #[derive(serde::Deserialize)]
    struct Fixture {
        cases: Vec<Row>,
    }
    #[derive(serde::Deserialize)]
    struct Input {
        pieces: Vec<Piece>,
        max_width: Option<f64>,
        authored: bool,
        size: [f64; 2],
        leading: f64,
        align: usize,
        baseline: usize,
    }
    #[derive(serde::Deserialize)]
    struct Expected {
        size: [f64; 2],
        tokens: Vec<Token>,
    }
    #[derive(serde::Deserialize)]
    struct Token {
        line: usize,
        x: i32,
        y: i32,
        bounds: [i32; 4],
    }
    #[derive(serde::Deserialize)]
    struct Row {
        input: Input,
        expected: Expected,
    }
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/text-layout.json")).unwrap();
    for (index, row) in fixture.cases.into_iter().enumerate() {
        let p = row.input;
        let actual = layout(
            &p.pieces,
            p.max_width.unwrap_or(f64::INFINITY),
            p.authored,
            p.size,
            p.leading,
            [Align::Left, Align::Center, Align::Right][p.align],
            [
                Baseline::Top,
                Baseline::Middle,
                Baseline::Bottom,
                Baseline::Alphabetic,
            ][p.baseline],
        )
        .unwrap();
        assert_eq!(actual.size, row.expected.size, "case {index}");
        assert_eq!(
            actual.tokens.len(),
            row.expected.tokens.len(),
            "case {index}"
        );
        for (actual, expected) in actual.tokens.iter().zip(row.expected.tokens) {
            assert_eq!(
                (actual.line, actual.x, actual.y, actual.bounds),
                (expected.line, expected.x, expected.y, expected.bounds),
                "case {index}"
            );
        }
    }
}
