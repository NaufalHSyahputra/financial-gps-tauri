import DotsThreeVertical from 'phosphor-icons-svelte/IconDotsThreeVerticalRegular.svelte';
import PencilSimple from 'phosphor-icons-svelte/IconPencilSimpleRegular.svelte';
import Trash from 'phosphor-icons-svelte/IconTrashRegular.svelte';
import ArchiveFill from 'phosphor-icons-svelte/IconArchiveFill.svelte';
import ArchiveRegular from 'phosphor-icons-svelte/IconArchiveRegular.svelte';
import ArrowDownLeftRegular from 'phosphor-icons-svelte/IconArrowDownLeftRegular.svelte';
import ArrowLeftRegular from 'phosphor-icons-svelte/IconArrowLeftRegular.svelte';
import ArrowUpRightRegular from 'phosphor-icons-svelte/IconArrowUpRightRegular.svelte';
import ArrowsClockwiseFill from 'phosphor-icons-svelte/IconArrowsClockwiseFill.svelte';
import ArrowsClockwiseRegular from 'phosphor-icons-svelte/IconArrowsClockwiseRegular.svelte';
import BankDuotone from 'phosphor-icons-svelte/IconBankDuotone.svelte';
import BookOpenRegular from 'phosphor-icons-svelte/IconBookOpenRegular.svelte';
import BuildingsFill from 'phosphor-icons-svelte/IconBuildingsFill.svelte';
import BuildingsRegular from 'phosphor-icons-svelte/IconBuildingsRegular.svelte';
import CalendarDotsFill from 'phosphor-icons-svelte/IconCalendarDotsFill.svelte';
import CalendarDotsRegular from 'phosphor-icons-svelte/IconCalendarDotsRegular.svelte';
import ChartBarFill from 'phosphor-icons-svelte/IconChartBarFill.svelte';
import ChartBarRegular from 'phosphor-icons-svelte/IconChartBarRegular.svelte';
import ChartPieSliceFill from 'phosphor-icons-svelte/IconChartPieSliceFill.svelte';
import ChartPieSliceRegular from 'phosphor-icons-svelte/IconChartPieSliceRegular.svelte';
import CreditCardDuotone from 'phosphor-icons-svelte/IconCreditCardDuotone.svelte';
import DotsThreeFill from 'phosphor-icons-svelte/IconDotsThreeFill.svelte';
import DotsThreeRegular from 'phosphor-icons-svelte/IconDotsThreeRegular.svelte';
import EyeRegular from 'phosphor-icons-svelte/IconEyeRegular.svelte';
import EyeSlashRegular from 'phosphor-icons-svelte/IconEyeSlashRegular.svelte';
import GearFill from 'phosphor-icons-svelte/IconGearFill.svelte';
import GearRegular from 'phosphor-icons-svelte/IconGearRegular.svelte';
import HandCoinsDuotone from 'phosphor-icons-svelte/IconHandCoinsDuotone.svelte';
import HouseFill from 'phosphor-icons-svelte/IconHouseFill.svelte';
import HouseRegular from 'phosphor-icons-svelte/IconHouseRegular.svelte';
import ListBulletsFill from 'phosphor-icons-svelte/IconListBulletsFill.svelte';
import ListBulletsRegular from 'phosphor-icons-svelte/IconListBulletsRegular.svelte';
import MagnifyingGlassRegular from 'phosphor-icons-svelte/IconMagnifyingGlassRegular.svelte';
import MoneyDuotone from 'phosphor-icons-svelte/IconMoneyDuotone.svelte';
import PiggyBankDuotone from 'phosphor-icons-svelte/IconPiggyBankDuotone.svelte';
import PlusRegular from 'phosphor-icons-svelte/IconPlusRegular.svelte';
import SlidersHorizontalFill from 'phosphor-icons-svelte/IconSlidersHorizontalFill.svelte';
import SlidersHorizontalRegular from 'phosphor-icons-svelte/IconSlidersHorizontalRegular.svelte';
import TagFill from 'phosphor-icons-svelte/IconTagFill.svelte';
import TagRegular from 'phosphor-icons-svelte/IconTagRegular.svelte';
import TargetFill from 'phosphor-icons-svelte/IconTargetFill.svelte';
import TargetRegular from 'phosphor-icons-svelte/IconTargetRegular.svelte';
import UsersFill from 'phosphor-icons-svelte/IconUsersFill.svelte';
import UsersRegular from 'phosphor-icons-svelte/IconUsersRegular.svelte';
import WalletDuotone from 'phosphor-icons-svelte/IconWalletDuotone.svelte';
import WalletFill from 'phosphor-icons-svelte/IconWalletFill.svelte';
import WalletRegular from 'phosphor-icons-svelte/IconWalletRegular.svelte';
import XRegular from 'phosphor-icons-svelte/IconXRegular.svelte';

export { default as CheckCircleFill } from 'phosphor-icons-svelte/IconCheckCircleFill.svelte';

export const navIcons = {
  home: [HouseRegular, HouseFill],
  activity: [ListBulletsRegular, ListBulletsFill],
  plan: [ChartPieSliceRegular, ChartPieSliceFill],
  more: [DotsThreeRegular, DotsThreeFill],
  accounts: [WalletRegular, WalletFill],
  calendar: [CalendarDotsRegular, CalendarDotsFill],
  report: [ChartBarRegular, ChartBarFill],
  rules: [SlidersHorizontalRegular, SlidersHorizontalFill],
  category: [TagRegular, TagFill],
  people: [UsersRegular, UsersFill],
  recurring: [ArrowsClockwiseRegular, ArrowsClockwiseFill],
  goal: [TargetRegular, TargetFill],
  assets: [BuildingsRegular, BuildingsFill],
  settings: [GearRegular, GearFill],
  backup: [ArchiveRegular, ArchiveFill],
} as const;

export const rowActionIcons = { edit: PencilSimple, delete: Trash, more: DotsThreeVertical } as const;

export const actionIcons = {
  add: PlusRegular,
  back: ArrowLeftRegular,
  close: XRegular,
  ledger: BookOpenRegular,
  eye: EyeRegular,
  eyeOff: EyeSlashRegular,
  search: MagnifyingGlassRegular,
  in: ArrowDownLeftRegular,
  out: ArrowUpRightRegular,
} as const;

export const accountIcons = {
  cash: MoneyDuotone,
  checking: BankDuotone,
  savings: PiggyBankDuotone,
  creditCard: CreditCardDuotone,
  loan: HandCoinsDuotone,
  wallet: WalletDuotone,
} as const;

export const routeIcons = {
  dashboard: 'home',
  transactions: 'activity',
  accounts: 'accounts',
  calendar: 'calendar',
  reports: 'report',
  rules: 'rules',
  categories: 'category',
  payees: 'people',
  planning: 'plan',
  budgets: 'plan',
  recurring: 'recurring',
  goals: 'goal',
  assets: 'assets',
  groups: 'people',
  settings: 'settings',
  backups: 'backup',
} as const satisfies Record<string, keyof typeof navIcons>;

export function routeIcon(href: string) {
  const key = href.startsWith('/settings/backups') ? 'backups' : href.split('/')[1];
  return routeIcons[key as keyof typeof routeIcons] ?? 'more';
}
