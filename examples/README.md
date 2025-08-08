# mpool Test Examples

This directory contains examples demonstrating the mpool connection pool functionality.

## Examples

### 1. `mpool_test.rs` - Complete Connection Pool Test

A comprehensive test of the mpool functionality with real TCP connections.

**Features:**
- Real TCP connection pool testing
- Concurrent connection handling
- Performance benchmarks
- Pool statistics monitoring
- Connection health checks
- Error handling demonstration

**Usage:**
```bash
# Terminal 1: Start the echo server
cargo run --example echo_server

# Terminal 2: Run the connection pool test
cargo run --example mpool_test
```

### 2. `echo_server.rs` - Simple Echo Server

A basic TCP echo server for testing connection pools.

**Features:**
- Listens on `127.0.0.1:8080`
- Echoes back all received messages
- Handles multiple concurrent connections
- Provides connection logging

### 3. `mpool_explanation.rs` - Educational Example

A step-by-step demonstration of mpool concepts and usage patterns.

**Features:**
- Explains connection pool lifecycle
- Shows pool state transitions
- Demonstrates connection reuse
- Illustrates connection limits
- Mock connection implementation

## Test Scenarios

### Connection Pool Limits
The test creates a pool with `max_size=5` and attempts 15 concurrent operations to demonstrate:
- Connection reuse when available
- Pool limit enforcement
- Graceful error handling when limits exceeded

### Performance Testing
Runs 100 operations across 20 concurrent tasks to measure:
- Operations per second
- Connection pool efficiency
- Resource utilization

### Health Checking
Demonstrates automatic connection health monitoring:
- Background health check tasks
- Invalid connection detection
- Automatic connection replacement

## Expected Output

When running with the echo server:

```
🚀 Starting mpool TCP connection test
📊 Pool created with max_size=5
✅ Server connection test passed

🔄 Running concurrent connection tests...
✅ Task 0: Echo successful - 'Hello from task 0'
✅ Task 1: Echo successful - 'Hello from task 1'
...
❌ Task 10: Echo failed - exceed limit
...

📈 Test Results Summary:
   ✅ Successful operations: 5
   ❌ Failed operations: 10
   📊 Success rate: 33.3%

⚡ Running performance test...
   ⏱️  Completed 100 operations in 2.45s
   📊 Average: 40.8 ops/sec

📊 Final pool statistics:
   Active connections: 5
   Idle connections: 5
   Max pool size: 5

✨ mpool test completed successfully!
```

## Learning Outcomes

After running these examples, you'll understand:

1. **Connection Pool Lifecycle**: How connections are created, reused, and cleaned up
2. **Concurrency Control**: How pools limit resource usage and handle contention
3. **Performance Benefits**: The efficiency gains from connection reuse
4. **Error Handling**: Graceful degradation when resources are exhausted
5. **Health Monitoring**: Automatic detection and replacement of failed connections

## Troubleshooting

### "Connection refused" errors
Make sure the echo server is running:
```bash
cargo run --example echo_server
```

### "exceed limit" errors
This is expected behavior when more tasks request connections than the pool size allows. It demonstrates the pool's resource limiting functionality.

### Performance variations
Network performance can vary based on system load. The examples are designed to show relative performance patterns rather than absolute numbers.
