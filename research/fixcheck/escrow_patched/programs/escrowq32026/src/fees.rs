// FIXCHECK ONLY (research/fixcheck): candidate fix B, harvest the vault's withheld
// Token-2022 transfer fees to the mint right before the vault is closed.
use anchor_lang::prelude::*;
use anchor_spl::token_2022::spl_token_2022::{
    self,
    extension::{transfer_fee::TransferFeeAmount, BaseStateWithExtensions, StateWithExtensions},
};

pub fn harvest_vault_if_withheld<'info>(
    token_program: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
) -> Result<()> {
    if token_program.key() != spl_token_2022::ID {
        return Ok(());
    }
    let withheld = {
        let data = vault.try_borrow_data()?;
        let state = StateWithExtensions::<spl_token_2022::state::Account>::unpack(&data)?;
        state
            .get_extension::<TransferFeeAmount>()
            .map(|e| u64::from(e.withheld_amount))
            .unwrap_or(0)
    };
    if withheld == 0 {
        return Ok(());
    }
    let ix = spl_token_2022::extension::transfer_fee::instruction::harvest_withheld_tokens_to_mint(
        token_program.key,
        mint.key,
        &[vault.key],
    )?;
    anchor_lang::solana_program::program::invoke(
        &ix,
        &[mint.clone(), vault.clone(), token_program.clone()],
    )?;
    Ok(())
}
