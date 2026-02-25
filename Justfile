llvm_cov := "/opt/homebrew/Cellar/llvm/20.1.7/bin/llvm-cov"
llvm_profdata := "/opt/homebrew/Cellar/llvm/20.1.7/bin/llvm-profdata"

build:
    cargo build

test:
    cargo test

coverage:
    LLVM_COV={{llvm_cov}} LLVM_PROFDATA={{llvm_profdata}} cargo llvm-cov --open

integration-test:
    cargo test -p transdb-integration-tests
