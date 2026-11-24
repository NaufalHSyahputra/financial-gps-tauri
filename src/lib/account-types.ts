import type { AccountKind } from './drivers/runtime';

export const accountKinds: Record<AccountKind, { label: string; description: string; example: string }> = {
  cash: { label: 'Cash', description: 'Physical cash in your wallet', example: 'Cash Wallet' },
  checking: { label: 'Bank account', description: 'Money you use for everyday spending', example: 'Main Bank' },
  savings: { label: 'Savings', description: 'Money set aside for later', example: 'My Savings' },
  creditCard: { label: 'Credit card', description: 'Track spending you pay for later', example: 'My Credit Card' },
  loan: { label: 'Loan', description: 'Keep track of money you borrowed', example: 'Car Loan' }
};

export const accountOptions = [
  { kind: 'cash', ...accountKinds.cash },
  { kind: 'checking', ...accountKinds.checking },
  { kind: 'checking', label: 'Digital wallet', description: 'Money in a payment app', example: 'My Digital Wallet', wallet: true },
  { kind: 'savings', ...accountKinds.savings },
  { kind: 'creditCard', ...accountKinds.creditCard },
  { kind: 'loan', ...accountKinds.loan }
] satisfies { kind: AccountKind; label: string; description: string; example: string; wallet?: boolean }[];

export const setupCurrencies = [
  { code: 'IDR', scale: 0, name: 'Indonesian Rupiah' }, { code: 'JPY', scale: 0, name: 'Japanese Yen' },
  { code: 'KRW', scale: 0, name: 'South Korean Won' }, { code: 'USD', scale: 2, name: 'US Dollar' },
  { code: 'CNY', scale: 2, name: 'Chinese Yuan' }, { code: 'EUR', scale: 2, name: 'Euro' },
  { code: 'SGD', scale: 2, name: 'Singapore Dollar' }, { code: 'GBP', scale: 2, name: 'Pound Sterling' },
  { code: 'AUD', scale: 2, name: 'Australian Dollar' }, { code: 'KWD', scale: 3, name: 'Kuwaiti Dinar' },
  { code: 'BHD', scale: 3, name: 'Bahraini Dinar' }, { code: 'OMR', scale: 3, name: 'Omani Rial' }
];
