//! Reads the palettes straight out of ui/theme.slint and checks that every theme keeps its
//! text readable, so a colour tweak that makes something hard to read fails a test.

use std::collections::HashMap;

const THEME: &str = include_str!("../ui/theme.slint");

fn channel(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn luminance(hex: &str) -> f64 {
    let n = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
    0.2126 * channel(n(0)) + 0.7152 * channel(n(2)) + 0.0722 * channel(n(4))
}

fn contrast(a: &str, b: &str) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// One map of "key" -> "rrggbb" per palette block (colours with transparency are skipped)
fn palettes() -> Vec<HashMap<String, String>> {
    let start = THEME.find("out property <[Colors]> palettes: [").expect("palettes list");
    let body = &THEME[start..];
    let body = &body[..body.find("];").expect("end of palettes")];
    body.split('{')
        .skip(1)
        .map(|block| {
            let block = &block[..block.find('}').unwrap()];
            let mut map = HashMap::new();
            for part in block.split(',') {
                if let Some((k, v)) = part.split_once(':') {
                    let v = v.trim();
                    if let Some(hex) = v.strip_prefix('#').filter(|h| h.len() == 6) {
                        map.insert(k.trim().to_string(), hex.to_string());
                    }
                }
            }
            map
        })
        .collect()
}

#[test]
fn every_theme_has_a_name_and_a_full_palette() {
    let ps = palettes();
    let names_line = THEME.lines().find(|l| l.contains("property <[string]> names")).expect("names");
    let names = names_line.matches('"').count() / 2;
    assert_eq!(ps.len() + 1, names, "one name for each palette, plus \"Windows\"");
    for (i, p) in ps.iter().enumerate() {
        for key in ["bg", "bg-alt", "surface", "border", "border-strong", "text", "text2", "text3", "accent", "accent-bg", "accent-fill", "danger", "success", "success-bg"] {
            assert!(p.contains_key(key), "palette {} is missing {key}", i + 1);
        }
        assert_ne!(p["bg"], p["bg-alt"], "palette {}: bg-alt must differ from bg (it forces repaints)", i + 1);
    }
}

#[test]
fn every_theme_keeps_its_text_readable() {
    for (i, p) in palettes().iter().enumerate() {
        let n = i + 1;
        let check = |what: &str, fg: &str, bg: &str, min: f64| {
            let c = contrast(&p[fg], &p[bg]);
            assert!(c >= min, "palette {n}: {what} is {c:.1}:1, needs {min}:1 ({fg} {} on {bg} {})", p[fg], p[bg]);
        };
        check("main text on the background", "text", "bg", 7.0);
        for bg in ["bg", "surface"] {
            check("secondary text", "text2", bg, 4.5);
            check("dim text", "text3", bg, 4.5);
        }
        check("accent text on the background", "accent", "bg", 4.5);
        check("accent text on its highlight", "accent", "accent-bg", 4.5);
        check("danger text", "danger", "bg", 4.5);
        let white = contrast("ffffff", &p["accent-fill"]);
        assert!(white >= 4.5, "palette {n}: white on the accent fill is {white:.1}:1");
    }
}
