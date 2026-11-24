import type { Money } from './drivers/runtime';

export function formatMinor(money: Money, scale: number | undefined, locale = 'en-US') {
  if (scale === undefined || !/^-?\d+$/.test(money.minor)) return '—';
  const value = BigInt(money.minor);
  const absolute = value < 0n ? -value : value;
  const unit = 10n ** BigInt(scale);
  const integer = new Intl.NumberFormat(locale).format(absolute / unit);
  const separator = new Intl.NumberFormat(locale).formatToParts(1.1).find(part => part.type === 'decimal')?.value ?? '.';
  const fraction = scale ? separator + (absolute % unit).toString().padStart(scale, '0') : '';
  return `${money.currency} ${value < 0n ? '−' : ''}${integer}${fraction}`;
}

export function toMinor(value: string, scale: number): string {
  const match = /^([+-]?)(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!match || (match[3]?.length ?? 0) > scale) throw new Error('Enter an amount with the currency’s supported decimal places.');
  const amount = BigInt(match[2] + (match[3] ?? '').padEnd(scale, '0'));
  return (match[1] === '-' ? -amount : amount).toString();
}

export function minorToInput(minor: string, scale: number) {
  const negative = minor.startsWith('-');
  const value = (negative ? minor.slice(1) : minor).padStart(scale + 1, '0');
  return (negative ? '-' : '') + (scale ? value.slice(0, -scale) + '.' + value.slice(-scale) : value);
}
