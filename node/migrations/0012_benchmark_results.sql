-- Benchmark runs. The reserved table from the baseline was never written;
-- each run is now stored whole as JSON (BenchmarkRunDto), with the columns
-- needed to list and recover them.
DROP TABLE IF EXISTS benchmark_results;

CREATE TABLE benchmark_results (
    id         TEXT PRIMARY KEY,
    started_at TEXT NOT NULL,
    status     TEXT NOT NULL,
    run        TEXT NOT NULL
);
