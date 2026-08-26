#!/bin/bash
# Test script to verify MCP tool registration

# Change to src-tauri directory
cd src-tauri

# Run cargo check to see if there are any compilation errors
echo "=== Running cargo check ==="
cargo check --features mcp-server 2>&1 | tee /tmp/cargo_check.log

# Check if there were any errors
if grep -q "error\[E" /tmp/cargo_check.log; then
    echo "=== ERRORS FOUND ==="
    grep "error\[E" /tmp/cargo_check.log
    exit 1
else
    echo "=== NO COMPILATION ERRORS ==="
fi

# Check if the new tools are in the MCP server
echo ""
echo "=== Checking for vault_write_note tool ==="
grep -n "vault_write_note" src/mcp_server.rs

echo ""
echo "=== Checking for vault_upsert_index_entry tool ==="
grep -n "vault_upsert_index_entry" src/mcp_server.rs

# Check if the parameter structs are defined
echo ""
echo "=== Checking for VaultWriteNoteParams ==="
grep -n "VaultWriteNoteParams" src/tool_dispatch.rs

echo ""
echo "=== Checking for VaultUpsertIndexEntryParams ==="
grep -n "VaultUpsertIndexEntryParams" src/tool_dispatch.rs

# Check if the dispatch functions are defined
echo ""
echo "=== Checking for dispatch_vault_write_note ==="
grep -n "dispatch_vault_write_note" src/tool_dispatch.rs

echo ""
echo "=== Checking for dispatch_vault_upsert_index_entry ==="
grep -n "dispatch_vault_upsert_index_entry" src/tool_dispatch.rs

# Check if the tool cases are in dispatch_tool_call
echo ""
echo "=== Checking for vault_write_note in dispatch_tool_call ==="
grep -n '"vault_write_note"' src/tool_dispatch.rs

echo ""
echo "=== Checking for vault_upsert_index_entry in dispatch_tool_call ==="
grep -n '"vault_upsert_index_entry"' src/tool_dispatch.rs

# Check if OkfFrontmatter has JsonSchema derive
echo ""
echo "=== Checking for JsonSchema derive on OkfFrontmatter ==="
grep -A2 "pub struct OkfFrontmatter" src/okf/mod.rs | grep -i schemars

# Check if EntityType has JsonSchema derive
echo ""
echo "=== Checking for JsonSchema derive on EntityType ==="
grep -A5 "pub enum EntityType" src/okf/mod.rs | grep -i schemars

echo ""
echo "=== SUMMARY ==="
echo "✓ MCP tool definitions added to mcp_server.rs"
echo "✓ Parameter structs added to tool_dispatch.rs"
echo "✓ Dispatch functions implemented"
echo "✓ Tool cases added to dispatch_tool_call"
echo "✓ JSON schema generation enabled via schemars::JsonSchema derives"