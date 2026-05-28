// Benchmarks for the grep utility
//
// SPDX-License-Identifier: MIT
//
// This file is part of the uutils grep package.
// It is licensed under the MIT License.
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

use divan::{Bencher, black_box};
use uu_grep::uumain;
use uucore::benchmark::{create_test_file, run_util_function};

/// Build an access-log-like data set with `n` lines.
///
/// Roughly a quarter of the lines use a non-default HTTP method / status /
/// user-agent so that selective patterns match a realistic subset rather than
/// every line or no line at all.
fn access_log(n: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for i in 0..n {
        let method = if i % 4 == 0 { "POST" } else { "GET" };
        let status = if i % 7 == 0 { 404 } else { 200 };
        let agent = if i % 3 == 0 {
            "Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0"
        } else {
            "curl/8.5.0"
        };
        let line = format!(
            "192.168.{}.{} - - [01/Jan/2024:00:00:00 +0000] \"{} /index.html HTTP/1.1\" {} 1234 \"-\" \"{}\"\n",
            (i / 256) % 256,
            i % 256,
            method,
            status,
            agent,
        );
        data.extend_from_slice(line.as_bytes());
    }
    data
}

/// Benchmark a literal search that matches nothing.
///
/// This is the purest measure of raw scan throughput: the whole file is read
/// and searched but no output is produced.
#[divan::bench]
fn literal_no_match(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(
            uumain,
            &["ZZZ_NONEXISTENT_PATTERN_ZZZ", file_path_str],
        ));
    });
}

/// Benchmark a literal search that matches a subset of lines.
#[divan::bench]
fn literal_match_some(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["POST", file_path_str]));
    });
}

/// Benchmark a literal search that matches every line (counting only).
///
/// `-c` keeps the output bounded so the benchmark measures matching rather than
/// terminal I/O.
#[divan::bench]
fn literal_match_all_count(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-c", "HTTP", file_path_str]));
    });
}

/// Benchmark a fixed-string search (`-F`).
#[divan::bench]
fn fixed_string(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(
            uumain,
            &["-F", "Chrome/120.0", file_path_str],
        ));
    });
}

/// Benchmark a case-insensitive search (`-i`).
#[divan::bench]
fn case_insensitive(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-i", "mozilla", file_path_str]));
    });
}

/// Benchmark counting matches (`-c`).
#[divan::bench]
fn count(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-c", "POST", file_path_str]));
    });
}

/// Benchmark an inverted match (`-v`).
///
/// Most lines do not contain "POST", so this selects the majority of lines;
/// `-c` bounds the output.
#[divan::bench]
fn invert_match_count(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-vc", "POST", file_path_str]));
    });
}

/// Benchmark printing line numbers (`-n`).
#[divan::bench]
fn line_number(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(1_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-nc", "POST", file_path_str]));
    });
}

/// Benchmark word-boundary matching (`-w`).
#[divan::bench]
fn word_match(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-wc", "GET", file_path_str]));
    });
}

/// Benchmark an extended regular expression with alternation (`-E`).
#[divan::bench]
fn extended_regex(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(
            uumain,
            &["-Ec", "(POST|DELETE|PUT)", file_path_str],
        ));
    });
}

/// Benchmark a basic regular expression with an anchor and character class.
#[divan::bench]
fn basic_regex(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(
            uumain,
            &["-c", "^192\\.168\\.[0-9]*\\.0 ", file_path_str],
        ));
    });
}

/// Benchmark a Perl-compatible regular expression (`-P`).
#[divan::bench]
fn perl_regex(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(
            uumain,
            &["-Pc", "\"\\d{3}\" \\d+", file_path_str],
        ));
    });
}

/// Benchmark `--only-matching` (`-o`) extracting a substring from each line.
#[divan::bench]
fn only_matching(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(1_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(
            uumain,
            &["-Eoc", "[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+", file_path_str],
        ));
    });
}

/// Benchmark quiet mode (`-q`), which can stop at the first match.
#[divan::bench]
fn quiet_first_match(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = create_test_file(&access_log(2_000_000), temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-q", "POST", file_path_str]));
    });
}

/// Benchmark searching short numeric lines (many small lines).
#[divan::bench]
fn short_lines(bencher: Bencher) {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut data = Vec::new();
    for i in 0..10_000_000 {
        data.extend_from_slice(format!("{i}\n").as_bytes());
    }
    let file_path = create_test_file(&data, temp_dir.path());
    let file_path_str = file_path.to_str().unwrap();

    bencher.bench(|| {
        black_box(run_util_function(uumain, &["-c", "999", file_path_str]));
    });
}

fn main() {
    divan::main();
}
