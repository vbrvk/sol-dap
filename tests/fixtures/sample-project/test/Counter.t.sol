// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import "../src/Counter.sol";

contract CounterTest {
    Counter public counter;

    function setUp() public {
        counter = new Counter();
        counter.setNumber(0);
    }

    function testIncrement() public {
        counter.increment();
        assert(counter.number() == 1);
    }

    function testSetNumber() public {
        counter.setNumber(42);
        assert(counter.number() == 42);
    }
}
