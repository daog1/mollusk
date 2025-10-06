use {
    mollusk_svm::mt::MolluskMt,
    solana_clock::Clock,
    solana_epoch_schedule::EpochSchedule,
    solana_rent::Rent,
    solana_slot_hashes::SlotHashes,
};

#[test]
fn test_get_sysvar() {
    let mollusk = MolluskMt::default();

    // Test getting clock sysvar
    let clock: Clock = mollusk.get_sysvar();
    assert_eq!(clock.slot, 0); // Default slot should be 0

    // Test getting epoch schedule sysvar
    let epoch_schedule: EpochSchedule = mollusk.get_sysvar();
    assert!(epoch_schedule.slots_per_epoch > 0);

    // Test getting rent sysvar
    let rent: Rent = mollusk.get_sysvar();
    assert!(rent.lamports_per_byte_year > 0);

    println!("✅ get_sysvar tests passed!");
}

#[test]
fn test_set_sysvar() {
    let mut mollusk = MolluskMt::default();

    // Test setting clock sysvar
    let mut new_clock: Clock = mollusk.get_sysvar();
    new_clock.slot = 42;
    new_clock.epoch = 1;
    mollusk.set_sysvar(&new_clock);

    // Verify the clock was updated
    let updated_clock: Clock = mollusk.get_sysvar();
    assert_eq!(updated_clock.slot, 42);
    assert_eq!(updated_clock.epoch, 1);

    // Test setting rent sysvar
    let mut new_rent: Rent = mollusk.get_sysvar();
    let original_lamports_per_byte = new_rent.lamports_per_byte_year;
    new_rent.lamports_per_byte_year = 12345;
    mollusk.set_sysvar(&new_rent);

    // Verify the rent was updated
    let updated_rent: Rent = mollusk.get_sysvar();
    assert_eq!(updated_rent.lamports_per_byte_year, 12345);
    assert_ne!(
        updated_rent.lamports_per_byte_year,
        original_lamports_per_byte
    );

    println!("✅ set_sysvar tests passed!");
}

#[test]
fn test_expire_blockhash() {
    let mut mollusk = MolluskMt::default();

    // Get initial slot hashes
    let initial_slot_hashes: SlotHashes = mollusk.get_sysvar();
    let initial_len = initial_slot_hashes.len();
    println!("{:?}", initial_slot_hashes);

    // Expire blockhash
    mollusk.expire_blockhash();

    // Get updated slot hashes
    let updated_slot_hashes: SlotHashes = mollusk.get_sysvar();
    let updated_len = updated_slot_hashes.len();

    // Should have added a new slot hash entry
    assert!(updated_len >= initial_len);

    // The slot hashes should be different
    let initial_first = initial_slot_hashes.first();
    let updated_first = updated_slot_hashes.first();

    // If both exist, they should be different (unless we're at the limit)
    if let (Some(initial), Some(updated)) = (initial_first, updated_first) {
        // At minimum, one of the slot or hash should be different
        assert!(initial.0 != updated.0 || initial.1 != updated.1);
    }

    println!("✅ expire_blockhash tests passed!");
}

#[test]
fn test_combined_functionality() {
    let mut mollusk = MolluskMt::default();

    // Test the combination of all functions

    // 1. Set a custom clock
    let mut clock: Clock = mollusk.get_sysvar();
    clock.slot = 100;
    clock.unix_timestamp = 1234567890;
    mollusk.set_sysvar(&clock);

    // 2. Expire blockhash (this should use the updated slot)
    mollusk.expire_blockhash();

    // 3. Verify the clock timestamp is still as we set it, but slot may have changed
    let final_clock: Clock = mollusk.get_sysvar();
    assert_eq!(final_clock.unix_timestamp, 1234567890);
    // expire_blockhash advances the slot by 1
    assert_eq!(final_clock.slot, 101);

    // 4. Verify slot hashes were updated
    let final_slot_hashes: SlotHashes = mollusk.get_sysvar();
    assert!(final_slot_hashes.len() > 0);

    println!("✅ Combined functionality tests passed!");
}

#[test]
fn test_warp_to_slot_integration() {
    let mut mollusk = MolluskMt::default();

    // Warp to a specific slot
    mollusk.warp_to_slot(500);

    // Verify the clock was updated
    let clock: Clock = mollusk.get_sysvar();
    assert_eq!(clock.slot, 500);

    // Now expire blockhash and verify it uses the warped slot
    mollusk.expire_blockhash();

    let slot_hashes: SlotHashes = mollusk.get_sysvar();

    // Should have slot hashes entries
    assert!(slot_hashes.len() > 0);

    // The most recent entry should be for slot 501 since expire_blockhash advances slot by 1
    if let Some((slot, _hash)) = slot_hashes.first() {
        assert!(slot >= &500); // Should be slot 500 or more
    }

    println!("✅ warp_to_slot integration tests passed!");
}

#[test]
fn test_minimum_balance_for_rent_exemption() {
    let mollusk = MolluskMt::default();

    // Test with 0 data length
    let min_balance_zero = mollusk.minimum_balance_for_rent_exemption(0);
    assert!(min_balance_zero >= 1); // Should be at least 1 lamport

    // Test with some data length
    let data_len = 100;
    let min_balance = mollusk.minimum_balance_for_rent_exemption(data_len);
    assert!(min_balance >= min_balance_zero); // Larger data should cost more or equal

    // Test with larger data length
    let larger_data_len = 1000;
    let min_balance_larger = mollusk.minimum_balance_for_rent_exemption(larger_data_len);
    assert!(min_balance_larger >= min_balance); // Larger data should cost more or equal

    println!("✅ minimum_balance_for_rent_exemption tests passed!");
}