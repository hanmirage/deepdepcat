/** Basic statistics over a number array. */
export function average(numbers) {
  if (numbers.length === 0) return 0;
  return numbers.reduce((a, b) => a + b, 0) / numbers.length;
}

export function total(numbers) {
  return numbers.reduce((a, b) => a + b, 0);
}
