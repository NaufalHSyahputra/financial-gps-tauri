type Fraction = { numerator: bigint; denominator: bigint };

function fraction(numerator: bigint, denominator: bigint): Fraction {
  if (denominator === 0n) throw new Error('You can’t divide by zero.');
  const sign = denominator < 0n ? -1n : 1n;
  let a = numerator < 0n ? -numerator : numerator;
  let b = denominator * sign;
  while (b) [a, b] = [b, a % b];
  return { numerator: numerator / a * sign, denominator: denominator / a * sign };
}

/** Evaluate decimal amounts as fractions, rounding only the result to minor units. */
export function calculateMinor(input: string, scale: number, math: boolean): string {
  const text = input.trim().replaceAll(',', '.').replaceAll('×', '*').replaceAll('÷', '/').replaceAll('−', '-');
  if (!text || text.length > 100) throw new Error('Enter an amount up to 100 characters.');
  const tokens = text.match(/(?:\d+(?:\.\d+)?|\.\d+)|[+*/-]|\s+/g) ?? [];
  if (tokens.join('') !== text) throw new Error('Enter numbers and basic math signs only.');
  const parts = tokens.filter(token => token.trim());
  let next = 0;
  function number(): Fraction {
    const negative = parts[next] === '-';
    if (math && (negative || parts[next] === '+')) next++;
    const token = parts[next++];
    if (!token || !/^(?:\d+(?:\.\d+)?|\.\d+)$/.test(token)) throw new Error('Enter a number after each math sign.');
    const [whole, decimals = ''] = token.split('.');
    if (whole.length + decimals.length > 18) throw new Error('Enter a smaller amount.');
    if (!math && decimals.length > scale) throw new Error(`This currency uses ${scale} decimal places.`);
    return fraction(BigInt(whole + decimals) * (negative ? -1n : 1n), 10n ** BigInt(decimals.length));
  }
  function product(): Fraction {
    let left = number();
    while (math && (parts[next] === '*' || parts[next] === '/')) {
      const operation = parts[next++];
      const right = number();
      left = operation === '*' ? fraction(left.numerator * right.numerator, left.denominator * right.denominator)
        : fraction(left.numerator * right.denominator, left.denominator * right.numerator);
    }
    return left;
  }
  let result = product();
  while (math && (parts[next] === '+' || parts[next] === '-')) {
    const sign = parts[next++] === '+' ? 1n : -1n;
    const right = product();
    result = fraction(result.numerator * right.denominator + sign * right.numerator * result.denominator,
      result.denominator * right.denominator);
  }
  if (next !== parts.length) throw new Error(math ? 'Check the math signs and numbers.' : 'Turn on Calculator to use math signs.');
  const scaled = result.numerator * 10n ** BigInt(scale);
  const absolute = scaled < 0n ? -scaled : scaled;
  const rounded = (absolute * 2n + result.denominator) / (2n * result.denominator);
  if (rounded > 9_000_000_000_000n) throw new Error('Enter a smaller amount.');
  return (scaled < 0n ? -rounded : rounded).toString();
}
