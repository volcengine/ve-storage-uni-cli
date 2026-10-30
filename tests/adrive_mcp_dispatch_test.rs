#[path = "common/adrive_mcp.rs"]
mod adrive_mcp;

#[test]
fn unified_adrive_mcp_executes_tools() {
    adrive_mcp::assert_adrive_mcp_dispatch(
        env!("CARGO_BIN_EXE_ve-storage-uni-cli"),
        &["ve-adrive"],
    );
}
