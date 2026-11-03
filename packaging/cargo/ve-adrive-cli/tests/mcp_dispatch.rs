#[path = "../../../../tests/common/adrive_mcp.rs"]
mod adrive_mcp;

#[test]
fn dedicated_adrive_mcp_executes_tools() {
    adrive_mcp::assert_adrive_mcp_dispatch(env!("CARGO_BIN_EXE_ve-adrive-cli"), &[]);
}
