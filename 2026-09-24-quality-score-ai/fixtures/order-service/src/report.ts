import type { Order } from "./order.ts";

export function summarizeByCategory(orders: Order[]): Record<string, number> {
  const result: Record<string, number> = {};
  for (const order of orders) {
    for (const item of order.items) {
      if (result[item.category] === undefined) {
        result[item.category] = 0;
      }
      result[item.category] += item.price * item.qty;
    }
  }
  return result;
}

export function summarizeByCountry(orders: Order[]): Record<string, number> {
  const result: Record<string, number> = {};
  for (const order of orders) {
    for (const item of order.items) {
      if (result[order.user.country] === undefined) {
        result[order.user.country] = 0;
      }
      result[order.user.country] += item.price * item.qty;
    }
  }
  return result;
}

export function summarizeByTier(orders: Order[]): Record<string, number> {
  const result: Record<string, number> = {};
  for (const order of orders) {
    for (const item of order.items) {
      if (result[order.user.tier] === undefined) {
        result[order.user.tier] = 0;
      }
      result[order.user.tier] += item.price * item.qty;
    }
  }
  return result;
}
