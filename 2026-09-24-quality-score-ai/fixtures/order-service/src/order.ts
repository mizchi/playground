export type Item = { sku: string; price: number; qty: number; category: string };
export type User = { id: string; tier: "guest" | "member" | "gold"; years: number; country: string };
export type Order = { user: User; items: Item[]; coupon?: string };

export function calculateMemberDiscount(user: User, subtotal: number): number {
  let rate = 0;
  if (user.tier === "member") {
    if (user.years >= 5) {
      rate = 0.1;
    } else if (user.years >= 2) {
      rate = 0.05;
    } else {
      rate = 0.02;
    }
  }
  const discount = subtotal * rate;
  return Math.round(discount * 100) / 100;
}

export function calculateGoldDiscount(user: User, subtotal: number): number {
  let rate = 0;
  if (user.tier === "gold") {
    if (user.years >= 5) {
      rate = 0.2;
    } else if (user.years >= 2) {
      rate = 0.15;
    } else {
      rate = 0.1;
    }
  }
  const discount = subtotal * rate;
  return Math.round(discount * 100) / 100;
}

export function calculateShipping(order: Order): number {
  let shipping = 0;
  let weight = 0;
  for (const item of order.items) {
    if (item.category === "book") {
      weight += 0.5 * item.qty;
    } else if (item.category === "electronics") {
      weight += 2 * item.qty;
    } else if (item.category === "furniture") {
      weight += 20 * item.qty;
    } else {
      weight += 1 * item.qty;
    }
  }
  if (order.user.country === "JP") {
    if (weight < 1) {
      shipping = 300;
    } else if (weight < 5) {
      shipping = 600;
    } else if (weight < 20) {
      shipping = 1200;
    } else {
      shipping = 3000;
    }
  } else {
    if (weight < 1) {
      shipping = 1500;
    } else if (weight < 5) {
      shipping = 3000;
    } else if (weight < 20) {
      shipping = 6000;
    } else {
      shipping = 15000;
    }
  }
  if (order.user.tier === "gold") {
    shipping = 0;
  } else if (order.user.tier === "member") {
    let subtotal = 0;
    for (const item of order.items) {
      subtotal += item.price * item.qty;
    }
    if (subtotal >= 10000) {
      shipping = 0;
    }
  }
  return shipping;
}

export function validateOrder(order: Order): string[] {
  const errors: string[] = [];
  if (!order.user) {
    errors.push("user is required");
  } else {
    if (!order.user.id) {
      errors.push("user.id is required");
    }
    if (order.user.years < 0) {
      errors.push("user.years must be >= 0");
    }
  }
  if (!order.items || order.items.length === 0) {
    errors.push("items must not be empty");
  } else {
    for (let i = 0; i < order.items.length; i++) {
      const item = order.items[i];
      if (!item.sku) {
        errors.push(`items[${i}].sku is required`);
      }
      if (item.price < 0) {
        errors.push(`items[${i}].price must be >= 0`);
      } else if (item.qty <= 0) {
        errors.push(`items[${i}].qty must be > 0`);
      } else {
        if (item.category === "furniture" && item.qty > 10) {
          errors.push(`items[${i}] furniture qty must be <= 10`);
        }
      }
    }
  }
  if (order.coupon !== undefined) {
    if (order.coupon.length !== 8) {
      errors.push("coupon must be 8 chars");
    } else if (!/^[A-Z0-9]+$/.test(order.coupon)) {
      errors.push("coupon must be uppercase alphanumeric");
    }
  }
  return errors;
}

export function calculateTotal(order: Order): number {
  const errors = validateOrder(order);
  if (errors.length > 0) {
    throw new Error(errors.join(", "));
  }
  let subtotal = 0;
  for (const item of order.items) {
    subtotal += item.price * item.qty;
  }
  let discount = 0;
  if (order.user.tier === "member") {
    discount = calculateMemberDiscount(order.user, subtotal);
  } else if (order.user.tier === "gold") {
    discount = calculateGoldDiscount(order.user, subtotal);
  }
  let couponDiscount = 0;
  if (order.coupon) {
    if (order.coupon.startsWith("HALF")) {
      couponDiscount = (subtotal - discount) * 0.5;
    } else if (order.coupon.startsWith("OFF1")) {
      couponDiscount = 1000;
    }
  }
  const shipping = calculateShipping(order);
  let total = subtotal - discount - couponDiscount + shipping;
  if (total < 0) {
    total = 0;
  }
  return Math.round(total * 100) / 100;
}
