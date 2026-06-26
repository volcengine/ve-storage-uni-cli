## ---- 1. 安装工具 ----

```bash
cargo install cross # Linux 交叉编译(需 Docker)
cargo install cargo-xwin # Windows MSVC 交叉编译
brew install llvm # cargo-xwin 需要的链接器
brew install zig
cargo install cargo-zigbuild
```

## ---- 2. 安装 target(macOS 原生 + Windows;Linux 用 cross 可省) ----

```bash
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin
rustup target add x86_64-pc-windows-msvc

# 下面两个若用 cross 可不装;用 cargo-zigbuild 才需要装
rustup target add x86_64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
```

## ---- 3. 编译 ----

### macOS(原生)

```bash
cargo build --release --target x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
```

### Linux(cross + Docker)

```bash
cross build --release --target x86_64-unknown-linux-gnu
cross build --release --target aarch64-unknown-linux-gnu
```

### Linux(cargo-zigbuild)

```bash
cargo zigbuild --release --target x86_64-unknown-linux-gnu
cargo zigbuild --release --target aarch64-unknown-linux-gnu
cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.17
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.17
```

### Windows(cargo-xwin)

```bash
cargo xwin build --release --target x86_64-pc-windows-msvc
```
