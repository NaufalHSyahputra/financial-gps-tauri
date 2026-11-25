import {
  commands,
  type CategoryKind,
  type DatabaseDriver,
  type DriverResult,
  type Scope,
} from '#lib/drivers/runtime';

export type PresetId = 'indonesia-personal' | 'global-essential' | 'blank';
export type PresetLocale = 'en' | 'id';

export type LocalizedText = {
  en: string;
  id: string;
};

export type PresetCategoryItem = {
  key: string;
  name: LocalizedText;
  kind: CategoryKind;
  icon: string;
};

export type PresetGroupItem = {
  key: string;
  name: LocalizedText;
  sortOrder: number;
  categories: PresetCategoryItem[];
};

export type CategoryPreset = {
  id: PresetId;
  name: LocalizedText;
  description: LocalizedText;
  groups: PresetGroupItem[];
};

export const categoryPresets: CategoryPreset[] = [
  {
    id: 'indonesia-personal',
    name: {
      en: 'Indonesia Personal Finance',
      id: 'Keuangan Pribadi Indonesia',
    },
    description: {
      en: '7 groups · 33 categories tailored for IDR cashflow, KPR/kos, PLN/PDAM, ojol, BPJS, zakat, and THR.',
      id: '7 grup · 33 kategori untuk arus kas harian, KPR/kos, PLN/PDAM, ojol, BPJS, zakat, dan THR.',
    },
    groups: [
      {
        key: 'income',
        name: { en: 'Income', id: 'Pemasukan' },
        sortOrder: 10,
        categories: [
          { key: 'primary-salary', name: { en: 'Primary Salary', id: 'Gaji Utama' }, kind: 'income', icon: 'banknote' },
          { key: 'bonuses-thr', name: { en: 'Bonuses & THR', id: 'Bonus & THR' }, kind: 'income', icon: 'gift' },
          { key: 'freelance-side', name: { en: 'Freelance & Side Hustles', id: 'Freelance & Sampingan' }, kind: 'income', icon: 'briefcase' },
          { key: 'cashback-interest', name: { en: 'Cashback, Interest & Dividends', id: 'Cashback, Bunga & Dividen' }, kind: 'income', icon: 'percent' },
        ],
      },
      {
        key: 'housing',
        name: { en: 'Housing & Utilities', id: 'Rumah & Utilitas' },
        sortOrder: 20,
        categories: [
          { key: 'rent-kpr', name: { en: 'Rent & Mortgage / KPR', id: 'Sewa, Kos & KPR' }, kind: 'expense', icon: 'home' },
          { key: 'electricity-water', name: { en: 'Electricity & Water', id: 'Listrik (PLN) & Air (PDAM)' }, kind: 'expense', icon: 'zap' },
          { key: 'internet-mobile', name: { en: 'Internet & Mobile Data', id: 'Internet, Pulsa & Paket Data' }, kind: 'expense', icon: 'wifi' },
          { key: 'ipl-fees', name: { en: 'Neighborhood & IPL Fees', id: 'Iuran Lingkungan & IPL' }, kind: 'expense', icon: 'building-2' },
          { key: 'home-maintenance', name: { en: 'Home Maintenance & Furnishings', id: 'Perawatan & Perabot Rumah' }, kind: 'expense', icon: 'wrench' },
        ],
      },
      {
        key: 'food',
        name: { en: 'Food & Daily Essentials', id: 'Makan & Kebutuhan Harian' },
        sortOrder: 30,
        categories: [
          { key: 'daily-meals', name: { en: 'Daily Meals & Takeout', id: 'Makan Harian & Jajan' }, kind: 'expense', icon: 'utensils' },
          { key: 'groceries-cooking', name: { en: 'Groceries & Cooking Ingredients', id: 'Belanja Bulanan & Bahan Masak' }, kind: 'expense', icon: 'shopping-basket' },
          { key: 'household-supplies', name: { en: 'Household & Cleaning Supplies', id: 'Kebutuhan Rumah & Kebersihan' }, kind: 'expense', icon: 'sparkles' },
          { key: 'personal-care', name: { en: 'Personal Care', id: 'Perawatan Diri' }, kind: 'expense', icon: 'scissors' },
        ],
      },
      {
        key: 'transport',
        name: { en: 'Transportation & Vehicles', id: 'Transportasi & Kendaraan' },
        sortOrder: 40,
        categories: [
          { key: 'transit-ojol', name: { en: 'Public Transit & Ride-Hailing', id: 'Transportasi Umum & Ojol' }, kind: 'expense', icon: 'train-front' },
          { key: 'fuel-bbm', name: { en: 'Fuel / BBM', id: 'Bensin & BBM' }, kind: 'expense', icon: 'fuel' },
          { key: 'tolls-parking', name: { en: 'Tolls & Parking', id: 'Tol & Parkir' }, kind: 'expense', icon: 'circle-parking' },
          { key: 'vehicle-service', name: { en: 'Vehicle Service & Tax', id: 'Servis & Pajak Kendaraan' }, kind: 'expense', icon: 'bike' },
        ],
      },
      {
        key: 'family-health',
        name: { en: 'Family, Social & Health', id: 'Keluarga, Sosial & Kesehatan' },
        sortOrder: 50,
        categories: [
          { key: 'family-parents', name: { en: 'Family & Parents Support', id: 'Keluarga & Orang Tua' }, kind: 'expense', icon: 'users' },
          { key: 'zakat-infaq', name: { en: 'Zakat, Infaq & Donations', id: 'Zakat, Infaq & Donasi' }, kind: 'expense', icon: 'hand-heart' },
          { key: 'kondangan-gifts', name: { en: 'Weddings & Gifts / Kondangan', id: 'Kondangan & Hadiah' }, kind: 'expense', icon: 'gift' },
          { key: 'healthcare-medicine', name: { en: 'Healthcare & Medicine', id: 'Kesehatan & Obat' }, kind: 'expense', icon: 'stethoscope' },
          { key: 'bpjs-insurance', name: { en: 'BPJS & Insurance', id: 'BPJS & Asuransi' }, kind: 'expense', icon: 'shield-check' },
          { key: 'pets', name: { en: 'Pets', id: 'Hewan Peliharaan' }, kind: 'expense', icon: 'paw-print' },
        ],
      },
      {
        key: 'lifestyle',
        name: { en: 'Lifestyle & Subscriptions', id: 'Gaya Hidup & Langganan' },
        sortOrder: 60,
        categories: [
          { key: 'coffee-hangouts', name: { en: 'Coffee & Hangouts', id: 'Kopi & Nongkrong' }, kind: 'expense', icon: 'coffee' },
          { key: 'entertainment-hobbies', name: { en: 'Entertainment & Hobbies', id: 'Hiburan & Hobi' }, kind: 'expense', icon: 'film' },
          { key: 'digital-subscriptions', name: { en: 'Digital Subscriptions', id: 'Langganan Digital' }, kind: 'expense', icon: 'cloud' },
          { key: 'clothing-accessories', name: { en: 'Clothing & Accessories', id: 'Pakaian & Aksesoris' }, kind: 'expense', icon: 'shirt' },
          { key: 'travel-vacations', name: { en: 'Travel & Vacations', id: 'Liburan & Perjalanan' }, kind: 'expense', icon: 'plane' },
        ],
      },
      {
        key: 'debt-goals',
        name: { en: 'Debt, Taxes & Future Goals', id: 'Cicilan, Pajak & Masa Depan' },
        sortOrder: 70,
        categories: [
          { key: 'installments-paylater', name: { en: 'Installments & PayLater', id: 'Cicilan & PayLater' }, kind: 'expense', icon: 'credit-card' },
          { key: 'taxes-admin', name: { en: 'Taxes & Admin Fees', id: 'Pajak & Biaya Admin' }, kind: 'expense', icon: 'landmark' },
          { key: 'emergency-fund', name: { en: 'Emergency Fund', id: 'Dana Darurat' }, kind: 'expense', icon: 'vault' },
          { key: 'investments', name: { en: 'Investments', id: 'Investasi' }, kind: 'expense', icon: 'trending-up' },
          { key: 'goal-savings', name: { en: 'Goal Savings', id: 'Tabungan Tujuan' }, kind: 'expense', icon: 'piggy-bank' },
        ],
      },
    ],
  },
  {
    id: 'global-essential',
    name: {
      en: 'Essential Global Starter',
      id: 'Paket Dasar Global',
    },
    description: {
      en: '5 groups · 18 universal categories for clean everyday budgeting in any currency.',
      id: '5 grup · 18 kategori ringkas untuk pencatatan keuangan sehari-hari.',
    },
    groups: [
      {
        key: 'income',
        name: { en: 'Income', id: 'Pemasukan' },
        sortOrder: 10,
        categories: [
          { key: 'salary', name: { en: 'Salary', id: 'Gaji' }, kind: 'income', icon: 'banknote' },
          { key: 'side-income', name: { en: 'Side Income & Bonus', id: 'Pendapatan Sampingan' }, kind: 'income', icon: 'coins' },
          { key: 'interest-refunds', name: { en: 'Interest & Refunds', id: 'Bunga & Pengembalian' }, kind: 'income', icon: 'percent' },
        ],
      },
      {
        key: 'housing-bills',
        name: { en: 'Housing & Bills', id: 'Rumah & Tagihan' },
        sortOrder: 20,
        categories: [
          { key: 'rent-mortgage', name: { en: 'Rent & Mortgage', id: 'Sewa & Cicilan Rumah' }, kind: 'expense', icon: 'home' },
          { key: 'utilities', name: { en: 'Utilities', id: 'Listrik, Air & Gas' }, kind: 'expense', icon: 'zap' },
          { key: 'internet-phone', name: { en: 'Internet & Phone', id: 'Internet & Telepon' }, kind: 'expense', icon: 'wifi' },
          { key: 'maintenance', name: { en: 'Home Maintenance', id: 'Perawatan Rumah' }, kind: 'expense', icon: 'wrench' },
        ],
      },
      {
        key: 'food-groceries',
        name: { en: 'Food & Groceries', id: 'Makanan & Belanja' },
        sortOrder: 30,
        categories: [
          { key: 'groceries', name: { en: 'Groceries', id: 'Belanja Bulanan' }, kind: 'expense', icon: 'shopping-cart' },
          { key: 'dining-out', name: { en: 'Dining Out', id: 'Makan di Luar' }, kind: 'expense', icon: 'utensils' },
          { key: 'coffee-snacks', name: { en: 'Coffee & Snacks', id: 'Kopi & Camilan' }, kind: 'expense', icon: 'coffee' },
        ],
      },
      {
        key: 'transport-health',
        name: { en: 'Transport & Health', id: 'Transportasi & Kesehatan' },
        sortOrder: 40,
        categories: [
          { key: 'transit-fuel', name: { en: 'Transit & Fuel', id: 'Transportasi & BBM' }, kind: 'expense', icon: 'car' },
          { key: 'healthcare', name: { en: 'Healthcare & Pharmacy', id: 'Kesehatan & Obat' }, kind: 'expense', icon: 'stethoscope' },
          { key: 'insurance', name: { en: 'Insurance', id: 'Asuransi' }, kind: 'expense', icon: 'shield-check' },
          { key: 'travel', name: { en: 'Travel', id: 'Perjalanan' }, kind: 'expense', icon: 'plane' },
        ],
      },
      {
        key: 'lifestyle-savings',
        name: { en: 'Lifestyle & Savings', id: 'Gaya Hidup & Tabungan' },
        sortOrder: 50,
        categories: [
          { key: 'shopping-clothing', name: { en: 'Shopping & Clothing', id: 'Belanja & Pakaian' }, kind: 'expense', icon: 'shirt' },
          { key: 'entertainment', name: { en: 'Entertainment & Subscriptions', id: 'Hiburan & Langganan' }, kind: 'expense', icon: 'film' },
          { key: 'gifts-donations', name: { en: 'Gifts & Donations', id: 'Hadiah & Donasi' }, kind: 'expense', icon: 'gift' },
          { key: 'savings-investments', name: { en: 'Savings & Investments', id: 'Tabungan & Investasi' }, kind: 'expense', icon: 'piggy-bank' },
        ],
      },
    ],
  },
  {
    id: 'blank',
    name: {
      en: 'Start Empty',
      id: 'Mulai Kosong',
    },
    description: {
      en: '0 groups · 0 categories. Build your own category structure from scratch.',
      id: '0 grup · 0 kategori. Susun kategori keuangan Anda sendiri dari awal.',
    },
    groups: [],
  },
];

export function getCategoryPreset(presetId: PresetId): CategoryPreset {
  return categoryPresets.find(preset => preset.id === presetId) ?? categoryPresets[0];
}

export function defaultGroupKeysForPreset(presetId: PresetId): string[] {
  return getCategoryPreset(presetId).groups.map(group => group.key);
}

type CatalogDriver = Pick<
  DatabaseDriver,
  'categoryGroupsList' | 'categoriesList' | 'categoryGroupSave' | 'categorySave'
>;

export type ApplyPresetInput = {
  scope: Scope;
  presetId: PresetId;
  locale?: PresetLocale;
  selectedGroupKeys?: string[];
  driver?: CatalogDriver;
};

export type ApplyPresetResult = {
  groupsCreated: number;
  categoriesCreated: number;
  skipped: number;
};

async function unwrap<T>(reply: Promise<DriverResult<T>>): Promise<T> {
  const result = await reply;
  if (result.status === 'error') throw result.error;
  return result.data;
}

const normalizeName = (value: string) => value.trim().toLowerCase();

export async function applyCategoryPreset({
  scope,
  presetId,
  locale = 'en',
  selectedGroupKeys,
  driver = commands,
}: ApplyPresetInput): Promise<ApplyPresetResult> {
  const preset = getCategoryPreset(presetId);
  if (!preset.groups.length) {
    return { groupsCreated: 0, categoriesCreated: 0, skipped: 0 };
  }

  const allowedGroups = selectedGroupKeys ? new Set(selectedGroupKeys) : null;
  const groupsToApply = preset.groups.filter(group => !allowedGroups || allowedGroups.has(group.key));
  if (!groupsToApply.length) {
    return { groupsCreated: 0, categoriesCreated: 0, skipped: 0 };
  }

  const existingGroups = new Map<string, string>();
  let groupCursor: string | null = null;
  do {
    const page = await unwrap(
      driver.categoryGroupsList(scope, { page: { cursor: groupCursor, limit: 200 } })
    );
    for (const row of page.rows) {
      existingGroups.set(normalizeName(row.draft.name), row.id);
    }
    groupCursor = page.nextCursor;
  } while (groupCursor);

  const existingCategories = new Set<string>();
  let categoryCursor: string | null = null;
  do {
    const page = await unwrap(
      driver.categoriesList(scope, { includeArchived: true, page: { cursor: categoryCursor, limit: 200 } })
    );
    for (const row of page.rows) {
      existingCategories.add(normalizeName(row.draft.name));
    }
    categoryCursor = page.nextCursor;
  } while (categoryCursor);

  let groupsCreated = 0;
  let categoriesCreated = 0;
  let skipped = 0;

  for (const group of groupsToApply) {
    const groupName = group.name[locale];
    const normalizedGroup = normalizeName(groupName);
    let groupId = existingGroups.get(normalizedGroup);

    if (!groupId) {
      const saved = await unwrap(
        driver.categoryGroupSave(
          { scope, requestId: crypto.randomUUID() },
          { target: null, draft: { name: groupName, sortOrder: group.sortOrder } }
        )
      );
      groupId = saved.data.id;
      existingGroups.set(normalizedGroup, groupId);
      groupsCreated++;
    } else {
      skipped++;
    }

    for (const category of group.categories) {
      const categoryName = category.name[locale];
      const normalizedCategory = normalizeName(categoryName);
      if (existingCategories.has(normalizedCategory)) {
        skipped++;
        continue;
      }
      await unwrap(
        driver.categorySave(
          { scope, requestId: crypto.randomUUID() },
          {
            target: null,
            draft: {
              name: categoryName,
              groupId,
              kind: category.kind,
              icon: category.icon,
              hidden: false,
              ignored: false,
              archived: false,
            },
          }
        )
      );
      existingCategories.add(normalizedCategory);
      categoriesCreated++;
    }
  }

  return { groupsCreated, categoriesCreated, skipped };
}
