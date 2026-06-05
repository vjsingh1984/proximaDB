# ProximaDB Spark JNI Binding

This crate builds `libproximadb_spark_jni` for the JVM-side Spark connector
smoke harness. It is a thin JNI wrapper around the Rust `spark_*` operations in
`src/connectors/spark.rs`.

## Lifecycle

`NativeProximaDB.initialize(dataDir)` creates one embedded ProximaDB instance per
JVM process. The native singleton is set once and cannot be replaced in the same
process. A second initialize call returns `false` and logs both the original and
requested data directories.

Spark or IDE class-loader reloads do not unload this native singleton. The cdylib
is process-scoped, not class-loader-scoped, so a reloaded Java class must keep
using the original initialized data directory.

Multiple JVM processes can load the cdylib independently. Coordination for a
shared data directory is handled by the embedded coordination layer
(`AccessMode::Exclusive`, `SharedRead`, and `LeaderFollower`) rather than by this
JNI crate.

`JNI_OnUnload` logs native-library unload and performs a best-effort
`EmbeddedProximaDB::flush()` if the singleton was initialized. JVMs normally keep
native libraries loaded until process exit, so this hook is not a reset
mechanism and should not be used for test isolation.

For JUnit migration, fork tests by class or process. Do not add a public reset
JNI method for normal connector use; a test-only reset hook is a separate
post-MVP design item if test runtime becomes a real problem.

## Local Smoke

From the repository root:

```sh
clients/jvm/spark-connector/run-smoke.sh
```

The script builds this cdylib, compiles the plain Java smoke harness, and runs
the JNI checks with `java.library.path` pointed at `target/debug`.

To verify cross-process coordination for the same embedded data directory:

```sh
bash crates/binding/proximadb-spark-jni/tests/cross_process_smoke.sh
```

The cross-process smoke starts one JVM that initializes and holds an exclusive
embedded instance, then starts a second JVM against the same directory. Success
means the second JVM cannot initialize concurrently (it may block on the file
lock or return `false`), and a third JVM can initialize after the first exits.

If `target/debug/libproximadb_spark_jni` is already built and another Cargo job
is holding the shared artifact lock, run it as:

```sh
SKIP_BUILD=1 bash crates/binding/proximadb-spark-jni/tests/cross_process_smoke.sh
```
