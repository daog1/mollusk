# MolluskMt Sysvar Functions

本文档描述了为 MolluskMt 新增的三个 sysvar 相关函数，这些函数参考了 litesvm 的实现。

## 新增函数

### 1. `get_sysvar<T>()`

获取指定类型的 sysvar 值。

**函数签名:**
```rust
pub fn get_sysvar<T>(&self) -> T
where
    T: Sysvar + SysvarId,
```

**支持的 Sysvar 类型:**
- `Clock` - 时钟信息（当前slot、epoch、时间戳等）
- `EpochSchedule` - Epoch 调度信息
- `EpochRewards` - Epoch 奖励信息
- `LastRestartSlot` - 上次重启的 slot
- `Rent` - 租金计算信息
- `SlotHashes` - 历史 slot 哈希
- `StakeHistory` - 质押历史

**使用示例:**
```rust
use mollusk_svm::mt::MolluskMt;
use solana_clock::Clock;
use solana_rent::Rent;

let mollusk = MolluskMt::default();

// 获取时钟 sysvar
let clock: Clock = mollusk.get_sysvar();
println!("Current slot: {}", clock.slot);
println!("Current epoch: {}", clock.epoch);

// 获取租金 sysvar
let rent: Rent = mollusk.get_sysvar();
println!("Lamports per byte year: {}", rent.lamports_per_byte_year);
```

### 2. `set_sysvar<T>()`

设置指定类型的 sysvar 值。

**函数签名:**
```rust
pub fn set_sysvar<T>(&mut self, sysvar: &T)
where
    T: Sysvar + SysvarId + Clone,
```

**使用示例:**
```rust
use mollusk_svm::mt::MolluskMt;
use solana_clock::Clock;

let mut mollusk = MolluskMt::default();

// 修改时钟 sysvar
let mut clock: Clock = mollusk.get_sysvar();
clock.slot = 100;
clock.unix_timestamp = 1234567890;
mollusk.set_sysvar(&clock);

// 验证修改
let updated_clock: Clock = mollusk.get_sysvar();
assert_eq!(updated_clock.slot, 100);
assert_eq!(updated_clock.unix_timestamp, 1234567890);
```

### 3. `expire_blockhash()`

使当前的 blockhash 过期，通过推进 slot 并添加新的 slot hash 条目来模拟区块链的推进。

**函数签名:**
```rust
pub fn expire_blockhash(&mut self)
```

**功能说明:**
- 将当前 slot 增加 1
- 为新 slot 生成新的哈希值
- 将新的 slot hash 条目添加到 SlotHashes sysvar 中

**使用示例:**
```rust
use mollusk_svm::mt::MolluskMt;
use solana_clock::Clock;
use solana_slot_hashes::SlotHashes;

let mut mollusk = MolluskMt::default();

// 获取初始状态
let initial_clock: Clock = mollusk.get_sysvar();
let initial_slot_hashes: SlotHashes = mollusk.get_sysvar();

println!("Initial slot: {}", initial_clock.slot);

// 使 blockhash 过期
mollusk.expire_blockhash();

// 检查更新后的状态
let updated_clock: Clock = mollusk.get_sysvar();
let updated_slot_hashes: SlotHashes = mollusk.get_sysvar();

println!("Updated slot: {}", updated_clock.slot); // 应该是 initial_slot + 1

// SlotHashes 应该包含新的条目
assert_ne!(
    initial_slot_hashes.first(),
    updated_slot_hashes.first()
);
```

## 完整示例

以下是一个完整的示例，展示如何结合使用这些功能：

```rust
use mollusk_svm::mt::MolluskMt;
use solana_clock::Clock;
use solana_rent::Rent;
use solana_slot_hashes::SlotHashes;

fn main() {
    let mut mollusk = MolluskMt::default();
    
    // 1. 获取并显示初始 sysvar 状态
    let clock: Clock = mollusk.get_sysvar();
    println!("Initial slot: {}, epoch: {}", clock.slot, clock.epoch);
    
    // 2. 修改时钟设置
    let mut new_clock = clock;
    new_clock.slot = 500;
    new_clock.unix_timestamp = 1640000000; // 2021-12-20
    mollusk.set_sysvar(&new_clock);
    
    // 3. 验证修改
    let updated_clock: Clock = mollusk.get_sysvar();
    println!("Updated slot: {}, timestamp: {}", 
             updated_clock.slot, updated_clock.unix_timestamp);
    
    // 4. 使 blockhash 过期
    mollusk.expire_blockhash();
    
    // 5. 检查最终状态
    let final_clock: Clock = mollusk.get_sysvar();
    let final_slot_hashes: SlotHashes = mollusk.get_sysvar();
    
    println!("Final slot: {} (expired blockhash advances slot by 1)", 
             final_clock.slot);
    println!("SlotHashes length: {}", final_slot_hashes.len());
    
    // 时间戳应该保持不变，但 slot 会增加
    assert_eq!(final_clock.unix_timestamp, 1640000000);
    assert_eq!(final_clock.slot, 501); // 500 + 1
}
```

## 与现有功能的集成

这些新函数与 MolluskMt 的现有功能很好地集成：

- 与 `warp_to_slot()` 配合使用可以进行时间旅行测试
- 与 `process_instruction()` 配合可以测试依赖特定 sysvar 状态的程序
- 支持在测试中模拟不同的区块链状态

## 注意事项

1. **类型安全**: 所有函数都是类型安全的，编译时会检查 sysvar 类型的有效性
2. **状态一致性**: 修改 sysvar 时要注意保持状态的一致性（例如，epoch 应该与 slot 匹配）
3. **测试隔离**: 在测试中使用这些函数时，确保每个测试都使用独立的 MolluskMt 实例
4. **性能**: 这些函数针对测试环境优化，不适用于生产环境

## 参考

这些函数的实现参考了 [LiteSVM](https://github.com/LiteSVM/litesvm) 项目的相应功能，提供了与 litesvm 类似的 API 接口。