#![cfg(feature = "runtime-benchmarks")]

extern crate alloc;

use super::*;
use alloc::{boxed::Box, vec::Vec};
use frame_benchmarking::v2::*;
use frame_support::{
	assert_ok,
	traits::{fungible::Inspect, schedule::DispatchTime, Get, StorePreimage},
};
use frame_system::RawOrigin;
use pallet_referenda::{BoundedCallOf, Pallet as Referenda, ReferendumCount, TracksInfo};
use pallet_referenda_precompiles::{IReferenda, ReferendaPrecompile};
use pallet_revive::{
	precompiles::{
		run::{precompile as run_precompile, CallSetup},
	},
	H160,
};
use scale_info::prelude::vec;
use sp_runtime::traits::Saturating;

fn funded_mapped_account<T: Config<I>, I: 'static>(name: &'static str, index: u32) -> T::AccountId {
	use frame_support::traits::fungible::Mutate;

	let account: T::AccountId = account(name, index, 0u32);

	// Calculate the mapping deposit: DepositPerByte * 52 + DepositPerItem
	let min_balance = <T as pallet_revive::Config>::Currency::minimum_balance();
	let deposit_per_byte = <T as pallet_revive::Config>::DepositPerByte::get();
	let deposit_per_item = <T as pallet_revive::Config>::DepositPerItem::get();
	let mapping_deposit =
		deposit_per_byte.saturating_mul(52u32.into()).saturating_add(deposit_per_item);

	// Fund enough to cover: minimum balance + mapping deposit + storage deposit limit + buffer
	let funding_amount = min_balance
		.saturating_add(mapping_deposit)
		.saturating_add(min_balance.saturating_mul(1000u32.into())) // storage deposit limit
		.saturating_add(min_balance.saturating_mul(100_000u32.into())); // buffer

	// Use set_balance to ensure the account has the required balance
	<T as pallet_revive::Config>::Currency::set_balance(&account, funding_amount);

	assert_ok!(pallet_revive::Pallet::<T>::map_account(RawOrigin::Signed(account.clone()).into()));

	account
}

/// Helper function to create a referendum (generic version for benchmarks)
/// Returns the referendum index
fn create_referendum_helper<T: Config<I> + pallet_referenda::Config<I>, I: 'static>(
	submitter: T::AccountId,
) -> u32 {
	let proposal_origin = Box::new(RawOrigin::Root.into());
	let inner_call = frame_system::Call::remark { remark: vec![] };
	let call = <T as pallet_referenda::Config<I>>::RuntimeCall::from(inner_call);
	let proposal: BoundedCallOf<T, I> =
		<T as pallet_referenda::Config<I>>::Preimages::bound(call).unwrap();
	let enactment_moment = DispatchTime::After(0u32.into());

	assert_ok!(Referenda::<T, I>::submit(
		RawOrigin::Signed(submitter).into(),
		proposal_origin,
		proposal,
		enactment_moment,
	));

	ReferendumCount::<T, I>::get() - 1
}

#[benchmarks]
mod benchmarks {
	use super::*;
	use codec::Encode;

	#[benchmark(pov_mode = Measured)]
	fn submission_deposit() {
		let call =
			IReferenda::IReferendaCalls::submissionDeposit(IReferenda::submissionDepositCall {});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn decision_deposit_not_found_or_completed() {
		// Case 1: Referendum doesn't exist (None) or is completed (Some(_) but not Ongoing)
		// Returns 0 immediately without track lookup
		// Code path: match referendum_info { None => 0u128, Some(_) => 0u128 }

		// Use a non-existent referendum index
		let non_existent_index = 999u32;

		let call = IReferenda::IReferendaCalls::decisionDeposit(IReferenda::decisionDepositCall {
			referendumIndex: non_existent_index,
		});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn decision_deposit_ongoing_no_deposit() {
		// Case 2: Referendum is Ongoing with decision_deposit.is_none()
		// This is the WORST CASE - requires track lookup
		// Code path: Some(Ongoing(status)) where status.decision_deposit.is_none()
		// Needs to: lookup referendum info, check deposit (None), lookup track info

		let submitter = funded_mapped_account::<T, ()>("submitter", 1);

		// Create referendum WITHOUT decision deposit
		let referendum_index = create_referendum_helper::<T, ()>(submitter);

		// The precompile will:
		// 1. Lookup referendum info (Ongoing status)
		// 2. Check decision_deposit (None)
		// 3. Lookup track info to get decision_deposit amount

		let call = IReferenda::IReferendaCalls::decisionDeposit(IReferenda::decisionDepositCall {
			referendumIndex: referendum_index,
		});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn decision_deposit_ongoing_with_deposit() {
		// Case 3: Referendum is Ongoing with decision_deposit.is_some()
		// Returns 0 without track lookup
		// Code path: Some(Ongoing(status)) where status.decision_deposit.is_some() => 0u128

		let submitter = funded_mapped_account::<T, ()>("submitter", 1);
		let depositor = funded_mapped_account::<T, ()>("depositor", 2);

		// Create referendum and place decision deposit
		use pallet_referenda::Pallet as Referenda;

		let referendum_index = create_referendum_helper::<T, ()>(submitter.clone());

		// Place decision deposit
		assert_ok!(Referenda::<T>::place_decision_deposit(
			RawOrigin::Signed(depositor.clone()).into(),
			referendum_index
		));

		// The precompile will:
		// 1. Lookup referendum info (Ongoing status)
		// 2. Check decision_deposit (Some) - returns 0
		// No track lookup needed

		let call = IReferenda::IReferendaCalls::decisionDeposit(IReferenda::decisionDepositCall {
			referendumIndex: referendum_index,
		});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn submit_inline_best_case() {
		// Best case: Empty queue, small proposal, simple origin

		// Simple origin (Root) - encode as PalletsOrigin
		use pallet_referenda::PalletsOriginOf;
		let proposal_origin: PalletsOriginOf<T> = RawOrigin::Root.into();
		let encoded_origin = proposal_origin.encode();

		// Small inline proposal (10 bytes)
		let proposal_data = (0..10).map(|_| 0u8).collect::<Vec<_>>();

		let call = IReferenda::IReferendaCalls::submitInline(IReferenda::submitInlineCall {
			origin: encoded_origin.into(),
			proposal: proposal_data.into(),
			timing: IReferenda::Timing::AfterBlock,
			enactmentMoment: 0u32,
		});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn submit_inline_worst_case() {
		// Worst case: Maximum proposal size (128 bytes)

		// Simple origin (Root) - encode as PalletsOrigin
		use pallet_referenda::PalletsOriginOf;
		let proposal_origin: PalletsOriginOf<T> = RawOrigin::Root.into();
		let encoded_origin = proposal_origin.encode();

		// Maximum inline proposal size (128 bytes)
		let proposal_data = (0..128).map(|_| 0u8).collect::<Vec<_>>();

		let call =
			IReferenda::IReferendaCalls::submitInline(IReferenda::submitInlineCall {
				origin: encoded_origin.into(),
				proposal: proposal_data.into(),
				timing: IReferenda::Timing::AfterBlock,
				enactmentMoment: 0u32,
			});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn submit_lookup_best_case() {
		// Best case: Empty queue, simple origin, small preimage length

		// Simple origin (Root) - encode as PalletsOrigin
		use pallet_referenda::PalletsOriginOf;
		let proposal_origin: PalletsOriginOf<T> = RawOrigin::Root.into();
		let encoded_origin = proposal_origin.encode();

		// Create a dummy hash (preimage doesn't need to exist at submission time)
		use sp_runtime::traits::Hash;
		let dummy_data = b"dummy_preimage";
		let hash = <T as frame_system::Config>::Hashing::hash(dummy_data);
		// Convert hash to [u8; 32] - hash implements AsRef<[u8]>
		let hash_bytes: [u8; 32] = hash.as_ref().try_into().unwrap_or_else(|_| {
			// Fallback: use encode if as_ref doesn't give us exactly 32 bytes
			let encoded = hash.encode();
			let mut bytes = [0u8; 32];
			bytes.copy_from_slice(&encoded[..32.min(encoded.len())]);
			bytes
		});

		let call = IReferenda::IReferendaCalls::submitLookup(IReferenda::submitLookupCall {
			origin: encoded_origin.into(),
			hash: hash_bytes.into(),
			preimageLength: 100u32, // Small preimage length
			timing: IReferenda::Timing::AfterBlock,
			enactmentMoment: 0u32,
		});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn submit_lookup_worst_case() {
		// Worst case: Maximum preimage length parameter

		// Simple origin (Root) - encode as PalletsOrigin
		use pallet_referenda::PalletsOriginOf;
		let proposal_origin: PalletsOriginOf<T> = RawOrigin::Root.into();
		let encoded_origin = proposal_origin.encode();

		// Create a dummy hash (preimage doesn't need to exist at submission time)
		use sp_runtime::traits::Hash;
		let dummy_data = b"dummy_preimage_large";
		let hash = <T as frame_system::Config>::Hashing::hash(dummy_data);
		// Convert hash to [u8; 32] - hash implements AsRef<[u8]>
		let hash_bytes: [u8; 32] = hash.as_ref().try_into().unwrap_or_else(|_| {
			// Fallback: use encode if as_ref doesn't give us exactly 32 bytes
			let encoded = hash.encode();
			let mut bytes = [0u8; 32];
			bytes.copy_from_slice(&encoded[..32.min(encoded.len())]);
			bytes
		});

		// Maximum preimage length (u32::MAX would be too large, use a large reasonable value)
		let max_preimage_length = 1_000_000u32;

		let call = IReferenda::IReferendaCalls::submitLookup(IReferenda::submitLookupCall {
			origin: encoded_origin.into(),
			hash: hash_bytes.into(),
			preimageLength: max_preimage_length,
			timing: IReferenda::Timing::AfterBlock,
			enactmentMoment: 0u32,
		});

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn place_decision_deposit_best_case() {
		// Best case: Referendum in AwaitingDeposit phase (simple state)
		let submitter = funded_mapped_account::<T, ()>("submitter", 1);

		// Create referendum WITHOUT decision deposit
		let referendum_index = create_referendum_helper::<T, ()>(submitter);

		let call = IReferenda::IReferendaCalls::placeDecisionDeposit(
			IReferenda::placeDecisionDepositCall { referendumIndex: referendum_index },
		);

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	#[benchmark(pov_mode = Measured)]
	fn place_decision_deposit_worst_case() {
		// Worst case: Place deposit when referendum is ready to start deciding immediately
		// This triggers BeginDecidingPassing/Failing branch (complex state transition)
		//
		// Note: The precompile calls env.charge() with max weight BEFORE executing:
		//   let max_weight = Weight::zero()
		//       .max(place_decision_deposit_preparing())  // ~45M
		//       .max(place_decision_deposit_queued())     // ~65M
		//       .max(place_decision_deposit_not_queued()) // ~66M (heaviest)
		//       .max(place_decision_deposit_passing())    // ~53M
		//       .max(place_decision_deposit_failing());    // ~51M
		//   env.charge(max_weight)?;
		//
		// This benchmark measures the full execution path including:
		//   1. env.charge() overhead (max weight calculation + gas meter update)
		//   2. Actual pallet execution (BeginDecidingPassing/Failing branch)
		// Users always pay max weight (~66M), but execution time varies by branch
		let submitter = funded_mapped_account::<T, ()>("submitter", 1);

		use pallet_referenda::Pallet as Referenda;
		use sp_runtime::traits::BlockNumberProvider;

		// Create referendum
		let referendum_index = create_referendum_helper::<T, ()>(submitter.clone());

		// Get prepare period and advance blocks so referendum is ready to start deciding
		let status = Referenda::<T>::ensure_ongoing(referendum_index).unwrap();
		let track = <T as pallet_referenda::Config<()>>::Tracks::info(status.track).unwrap();
		let prepare_period = track.prepare_period;

		// Advance blocks past prepare period so it's ready to start deciding
		let submitted = status.submitted;
		let target_block = submitted.saturating_add(prepare_period);
		<T as pallet_referenda::Config<()>>::BlockNumberProvider::set_block_number(target_block);

		// Now place deposit - this will trigger service_referendum which will
		// result in BeginDecidingPassing or BeginDecidingFailing branch (most complex)
		let call = IReferenda::IReferendaCalls::placeDecisionDeposit(
			IReferenda::placeDecisionDepositCall { referendumIndex: referendum_index },
		);

		let mut call_setup = CallSetup::<T>::default();
		let (mut ext, _) = call_setup.ext();

		let result;
		#[block]
		{
			result = run_precompile::<ReferendaPrecompile<T>, _>(
				&mut ext,
				H160::from_low_u64_be(0xB0000).as_fixed_bytes(),
				&call,
			);
		}

		assert!(result.is_ok());
	}

	impl_benchmark_test_suite!(
		ReferendaPrecompilesBenchmarks,
		crate::mock::new_test_ext(),
		crate::mock::Test
	);
}
