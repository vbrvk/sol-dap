// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

// ============ Interfaces ============

interface IERC20 {
    function totalSupply() external view returns (uint256);
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

// ============ Abstract base contract ============

abstract contract Ownable {
    address public owner;

    error NotOwner();
    error ZeroAddress();

    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    constructor() {
        owner = msg.sender;
        emit OwnershipTransferred(address(0), msg.sender);
    }

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        emit OwnershipTransferred(owner, newOwner);
        owner = newOwner;
    }
}

// ============ Abstract with virtual/override pattern ============

abstract contract Pausable is Ownable {
    bool public paused;

    error ContractPaused();

    event Paused(address account);
    event Unpaused(address account);

    modifier whenNotPaused() {
        if (paused) revert ContractPaused();
        _;
    }

    function pause() external onlyOwner {
        paused = true;
        emit Paused(msg.sender);
    }

    function unpause() external onlyOwner {
        paused = false;
        emit Unpaused(msg.sender);
    }

    /// @dev Hook for subclasses to add custom pause validation.
    function _validateNotPaused() internal virtual {
        if (paused) revert ContractPaused();
    }
}

// ============ Simple ERC20 with inline assembly ============

contract SimpleToken is IERC20 {
    string public name;
    string public symbol;
    uint8 public decimals;
    uint256 internal _totalSupply;

    mapping(address => uint256) internal _balances;
    mapping(address => mapping(address => uint256)) internal _allowances;

    constructor(string memory name_, string memory symbol_, uint8 decimals_) {
        name = name_;
        symbol = symbol_;
        decimals = decimals_;
    }

    function totalSupply() external view override returns (uint256) {
        return _totalSupply;
    }

    function balanceOf(address account) external view override returns (uint256) {
        return _balances[account];
    }

    function transfer(address to, uint256 amount) external override returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external override returns (bool) {
        _allowances[msg.sender][spender] = amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external override returns (bool) {
        uint256 currentAllowance = _allowances[from][msg.sender];
        require(currentAllowance >= amount, "ERC20: insufficient allowance");
        unchecked {
            _allowances[from][msg.sender] = currentAllowance - amount;
        }
        _transfer(from, to, amount);
        return true;
    }

    function _transfer(address from, address to, uint256 amount) internal {
        require(from != address(0), "ERC20: transfer from zero");
        require(to != address(0), "ERC20: transfer to zero");
        require(_balances[from] >= amount, "ERC20: insufficient balance");
        unchecked {
            _balances[from] -= amount;
            _balances[to] += amount;
        }
    }

    function _mint(address to, uint256 amount) internal {
        require(to != address(0), "ERC20: mint to zero");
        _totalSupply += amount;
        _balances[to] += amount;
    }

    /// @dev Public mint for testing.
    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    /// @dev Uses inline assembly to read a storage slot directly.
    function rawBalanceOf(address account) external view returns (uint256 result) {
        // _balances is at slot 4 (after name, symbol, decimals, _totalSupply)
        // mapping slot = keccak256(abi.encode(key, slot))
        assembly {
            mstore(0x00, account)
            mstore(0x20, 4) // _balances mapping slot
            let slot := keccak256(0x00, 0x40)
            result := sload(slot)
        }
    }

    /// @dev Assembly-based safe addition with overflow check.
    function safeAdd(uint256 a, uint256 b) public pure returns (uint256 result) {
        assembly {
            result := add(a, b)
            if lt(result, a) {
                // Overflow: revert with "overflow"
                mstore(0x00, 0x08c379a000000000000000000000000000000000000000000000000000000000)
                mstore(0x04, 0x20)
                mstore(0x24, 0x08)
                mstore(0x44, "overflow")
                revert(0x00, 0x64)
            }
        }
    }
}

// ============ Vault: multi-inheritance, virtual/override, events, reverts ============

contract Vault is Pausable {
    SimpleToken public token;

    mapping(address => uint256) public deposits;
    uint256 public totalDeposits;

    uint256 public constant MAX_DEPOSIT = 1_000_000e18;
    uint256 public feePercent = 1; // 1%

    error DepositTooLarge(uint256 amount, uint256 max);
    error InsufficientDeposit(uint256 requested, uint256 available);
    error TransferFailed();

    event Deposited(address indexed user, uint256 amount, uint256 fee);
    event Withdrawn(address indexed user, uint256 amount);
    event FeeUpdated(uint256 oldFee, uint256 newFee);

    constructor(address token_) {
        token = SimpleToken(token_);
    }

    /// @dev Override pause validation to also check total deposits.
    function _validateNotPaused() internal virtual override {
        super._validateNotPaused();
        // Additional check: emergency pause if deposits too high
    }

    function deposit(uint256 amount) external whenNotPaused {
        _validateNotPaused();
        if (amount > MAX_DEPOSIT) revert DepositTooLarge(amount, MAX_DEPOSIT);

        uint256 fee = _calculateFee(amount);
        uint256 netAmount = amount - fee;

        // Transfer tokens from user
        bool success = token.transferFrom(msg.sender, address(this), amount);
        if (!success) revert TransferFailed();

        deposits[msg.sender] += netAmount;
        totalDeposits += netAmount;

        emit Deposited(msg.sender, netAmount, fee);
    }

    function withdraw(uint256 amount) external whenNotPaused {
        if (deposits[msg.sender] < amount) {
            revert InsufficientDeposit(amount, deposits[msg.sender]);
        }

        deposits[msg.sender] -= amount;
        totalDeposits -= amount;

        bool success = token.transfer(msg.sender, amount);
        if (!success) revert TransferFailed();

        emit Withdrawn(msg.sender, amount);
    }

    function setFeePercent(uint256 newFee) external onlyOwner {
        require(newFee <= 10, "Fee too high");
        emit FeeUpdated(feePercent, newFee);
        feePercent = newFee;
    }

    function _calculateFee(uint256 amount) internal view returns (uint256) {
        return (amount * feePercent) / 100;
    }

    /// @dev Batch check balances using a loop (tests iteration debugging).
    function batchBalances(address[] calldata accounts) external view returns (uint256[] memory) {
        uint256[] memory results = new uint256[](accounts.length);
        for (uint256 i = 0; i < accounts.length; i++) {
            results[i] = deposits[accounts[i]];
        }
        return results;
    }
}

// ============ Callback pattern (reentrancy-like flow) ============

interface ICallback {
    function onDeposit(address user, uint256 amount) external;
}

contract VaultWithCallback is Vault {
    ICallback public callback;

    constructor(address token_, address callback_) Vault(token_) {
        callback = ICallback(callback_);
    }

    function depositWithCallback(uint256 amount) external whenNotPaused {
        _validateNotPaused();
        if (amount > MAX_DEPOSIT) revert DepositTooLarge(amount, MAX_DEPOSIT);

        uint256 fee = _calculateFee(amount);
        uint256 netAmount = amount - fee;

        bool success = token.transferFrom(msg.sender, address(this), amount);
        if (!success) revert TransferFailed();

        deposits[msg.sender] += netAmount;
        totalDeposits += netAmount;

        // External callback — cross-contract call for testing step-in/step-out
        callback.onDeposit(msg.sender, netAmount);

        emit Deposited(msg.sender, netAmount, fee);
    }
}
