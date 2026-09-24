import { test } from "node:test";
import assert from "node:assert/strict";
import { calculateTotal, calculateShipping, validateOrder, type Order } from "./order.ts";
import { summarizeByCategory, summarizeByCountry, summarizeByTier } from "./report.ts";

const guestJP: Order = {
  user: { id: "u1", tier: "guest", years: 0, country: "JP" },
  items: [{ sku: "b1", price: 1000, qty: 2, category: "book" }],
};
const memberUS: Order = {
  user: { id: "u2", tier: "member", years: 3, country: "US" },
  items: [
    { sku: "e1", price: 5000, qty: 1, category: "electronics" },
    { sku: "x1", price: 200, qty: 3, category: "misc" },
  ],
};
const goldJP: Order = {
  user: { id: "u3", tier: "gold", years: 6, country: "JP" },
  items: [{ sku: "f1", price: 30000, qty: 1, category: "furniture" }],
  coupon: "HALF0001",
};

test("shipping", () => {
  assert.equal(calculateShipping(guestJP), 600);
  assert.equal(calculateShipping(memberUS), 6000);
  assert.equal(calculateShipping(goldJP), 0);
  assert.equal(
    calculateShipping({ ...memberUS, items: [{ sku: "e", price: 20000, qty: 1, category: "electronics" }] }),
    0,
  );
  assert.equal(
    calculateShipping({ ...guestJP, items: [{ sku: "b", price: 1, qty: 1, category: "book" }] }),
    300,
  );
  assert.equal(
    calculateShipping({ ...guestJP, user: { ...guestJP.user, country: "FR" }, items: [{ sku: "f", price: 1, qty: 1, category: "furniture" }] }),
    15000,
  );
});

test("total", () => {
  assert.equal(calculateTotal(guestJP), 2600);
  assert.equal(calculateTotal(memberUS), 5600 - 280 + 6000);
  assert.equal(calculateTotal(goldJP), (30000 - 6000) * 0.5);
  assert.equal(calculateTotal({ ...guestJP, coupon: "OFF10000" }), 2000 - 1000 + 600);
  assert.equal(
    calculateTotal({ ...guestJP, items: [{ sku: "b", price: 100, qty: 1, category: "book" }], coupon: "OFF10000" }),
    0,
  );
  const newMember: Order = { ...memberUS, user: { ...memberUS.user, years: 1 } };
  assert.equal(calculateTotal(newMember), 5600 - 112 + 6000);
  const oldGold: Order = { ...goldJP, user: { ...goldJP.user, years: 3 }, coupon: undefined };
  assert.equal(calculateTotal(oldGold), 30000 - 4500);
});

test("validation", () => {
  assert.deepEqual(validateOrder(guestJP), []);
  assert.deepEqual(
    validateOrder({
      user: { id: "", tier: "guest", years: -1, country: "JP" },
      items: [
        { sku: "", price: -1, qty: 1, category: "book" },
        { sku: "a", price: 1, qty: 0, category: "book" },
        { sku: "b", price: 1, qty: 11, category: "furniture" },
      ],
      coupon: "abc",
    }),
    [
      "user.id is required",
      "user.years must be >= 0",
      "items[0].sku is required",
      "items[0].price must be >= 0",
      "items[1].qty must be > 0",
      "items[2] furniture qty must be <= 10",
      "coupon must be 8 chars",
    ],
  );
  assert.deepEqual(validateOrder({ ...guestJP, items: [] }), ["items must not be empty"]);
  assert.deepEqual(validateOrder({ ...guestJP, coupon: "abcdefgh" }), ["coupon must be uppercase alphanumeric"]);
  assert.throws(() => calculateTotal({ ...guestJP, items: [] }), /items must not be empty/);
});

test("report", () => {
  const orders = [guestJP, memberUS, goldJP];
  assert.deepEqual(summarizeByCategory(orders), { book: 2000, electronics: 5000, misc: 600, furniture: 30000 });
  assert.deepEqual(summarizeByCountry(orders), { JP: 32000, US: 5600 });
  assert.deepEqual(summarizeByTier(orders), { guest: 2000, member: 5600, gold: 30000 });
});
