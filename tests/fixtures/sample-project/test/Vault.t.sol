// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import "../src/Vault.sol";
import "forge-std/console.sol";

// ============ Mock callback for testing cross-contract calls ============

contract MockCallback is ICallback {
    uint256 public lastAmount;
    address public lastUser;
    uint256 public callCount;

    function onDeposit(address user, uint256 amount) external override {
        lastUser = user;
        lastAmount = amount;
        callCount++;
    }
}

// ============ Test contract: exercises all debug scenarios ============

contract VaultTest {
    SimpleToken public token;
    Vault public vault;
    VaultWithCallback public vaultCb;
    MockCallback public callback;

    address public alice = address(0xA11CE);
    address public bob = address(0xB0B);

    // ============ setUp ============

    function setUp() public {
        token = new SimpleToken("Test Token", "TST", 18);
        vault = new Vault(address(token));
        callback = new MockCallback();
        vaultCb = new VaultWithCallback(address(token), address(callback));
    }

    // ============ Basic deposit/withdraw flow ============
    // Tests: multi-step execution, storage changes, events

    function testDeposit() public {
        // Mint tokens to this contract
        token.mint(address(this), 1000e18);
        token.approve(address(vault), 1000e18);

        // Deposit — should deduct 1% fee
        vault.deposit(100e18);

        assert(vault.deposits(address(this)) == 99e18);
        assert(vault.totalDeposits() == 99e18);
    }

    function testWithdraw() public {
        token.mint(address(this), 1000e18);
        token.approve(address(vault), 1000e18);

        vault.deposit(100e18);
        vault.withdraw(50e18);

        assert(vault.deposits(address(this)) == 49e18);
        assert(token.balanceOf(address(this)) == 950e18);
    }

    // ============ Inheritance chain: Ownable -> Pausable -> Vault ============
    // Tests: modifier execution, base contract function calls, virtual/override

    function testPauseUnpause() public {
        vault.pause();
        assert(vault.paused() == true);

        vault.unpause();
        assert(vault.paused() == false);
    }

    function testOwnershipTransfer() public {
        vault.transferOwnership(alice);
        assert(vault.owner() == alice);
    }

    // ============ Cross-contract calls with callback ============
    // Tests: step-in across contracts, deep call chains

    function testDepositWithCallback() public {
        token.mint(address(this), 1000e18);
        token.approve(address(vaultCb), 1000e18);

        console.log(
            "Before deposit: balance =",
            token.balanceOf(address(this))
        );
        console.log("Depositing 100e18 into VaultWithCallback");

        vaultCb.depositWithCallback(100e18);

        console.log("After deposit: callback count =", callback.callCount());
        console.log("Callback last amount =", callback.lastAmount());
        console.log("Callback last user =", callback.lastUser());

        // Verify callback was called
        assert(callback.callCount() == 1);
        assert(callback.lastAmount() == 99e18);
        assert(callback.lastUser() == address(this));
        console.log("All assertions passed!");
    }

    // ============ Inline assembly ============
    // Tests: stepping through assembly blocks, raw storage reads

    function testRawBalanceOf() public {
        token.mint(alice, 500e18);

        uint256 raw = token.rawBalanceOf(alice);
        uint256 normal = token.balanceOf(alice);

        assert(raw == normal);
        assert(raw == 500e18);
    }

    function testSafeAdd() public {
        uint256 result = token.safeAdd(100, 200);
        assert(result == 300);
    }

    // ============ Loops and arrays ============
    // Tests: loop stepping, memory allocation

    function testBatchBalances() public {
        token.mint(address(this), 1000e18);
        token.approve(address(vault), 1000e18);

        vault.deposit(100e18);
        vault.deposit(200e18);

        address[] memory accounts = new address[](3);
        accounts[0] = address(this);
        accounts[1] = alice;
        accounts[2] = bob;

        uint256[] memory balances = vault.batchBalances(accounts);

        assert(balances[0] == 297e18); // 99 + 198, 1% fee each
        assert(balances[1] == 0);
        assert(balances[2] == 0);
    }

    // ============ Fee logic (internal function calls) ============
    // Tests: stepping into internal functions

    function testFeeCalculation() public {
        token.mint(address(this), 1000e18);
        token.approve(address(vault), 1000e18);

        // Default fee is 1%
        vault.deposit(1000e18);
        assert(vault.deposits(address(this)) == 990e18);

        // Change fee to 5%
        vault.setFeePercent(5);
        assert(vault.feePercent() == 5);
    }

    // ============ Error/revert paths ============
    // Tests: revert debugging, custom errors

    function testRevertDepositWhenPaused() public {
        vault.pause();
        token.mint(address(this), 100e18);
        token.approve(address(vault), 100e18);
        bool reverted = false;
        try vault.deposit(100e18) {
            // Should not reach here
        } catch {
            reverted = true;
        }
        assert(reverted);
    }

    function testRevertDepositTooLarge() public {
        uint256 tooMuch = 2_000_000e18;
        token.mint(address(this), tooMuch);
        token.approve(address(vault), tooMuch);
        bool reverted = false;
        try vault.deposit(tooMuch) {
            // Should not reach here
        } catch {
            reverted = true;
        }
        assert(reverted);
    }

    function testRevertWithdrawInsufficient() public {
        bool reverted = false;
        try vault.withdraw(100e18) {
            // Should not reach here
        } catch {
            reverted = true;
        }
        assert(reverted);
    }

    // ============ ERC20 transfer chain ============
    // Tests: deep call chain through transferFrom -> _transfer

    function testTokenTransferChain() public {
        token.mint(address(this), 500e18);
        token.approve(bob, 200e18);

        // Direct transfer
        token.transfer(alice, 100e18);
        assert(token.balanceOf(alice) == 100e18);
        assert(token.balanceOf(address(this)) == 400e18);
    }

    // ============ Multiple storage slot writes ============
    // Tests: tracking storage changes across many variables

    function testMultipleStorageWrites() public {
        token.mint(address(this), 10000e18);
        token.approve(address(vault), 10000e18);

        // Each deposit modifies: deposits[sender], totalDeposits, token balances
        vault.deposit(100e18);
        vault.deposit(200e18);
        vault.deposit(300e18);

        assert(vault.totalDeposits() == 594e18); // (99 + 198 + 297)
        assert(vault.deposits(address(this)) == 594e18);
    }
}
