use air_memory_mcp::daemon;

fn main() {
    // Real stdio loop lands in Task C3; this keeps the bin compiling against the lib.
    let _ = daemon::resolve_socket_path();
    eprintln!("air-memory-mcp: stub (loop lands in C3)");
}
