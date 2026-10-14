//! Terminal throughput: how fast output is parsed and the screen resolved.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rooste_terminal::Terminal;

/// About 1 MiB of typical output: colored `ls`-style lines and plain text.
fn sample() -> Vec<u8> {
    let line = "\x1b[1;34mdirectory\x1b[0m  \x1b[32mscript.sh\x1b[0m  plain-file.txt  \x1b[31marchive.tar.gz\x1b[0m  more text here\r\n";
    line.repeat(1024 * 1024 / line.len()).into_bytes()
}

fn throughput(c: &mut Criterion) {
    let data = sample();
    let mut group = c.benchmark_group("terminal");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("advance 1 MiB", |b| {
        b.iter(|| {
            let mut term = Terminal::new(200, 50);
            term.advance(&data);
            term
        })
    });
    group.finish();

    let mut term = Terminal::new(200, 50);
    term.advance(&data);
    c.bench_function("screen 200x50", |b| b.iter(|| term.screen()));
}

criterion_group!(benches, throughput);
criterion_main!(benches);
