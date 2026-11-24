import type { Component } from 'svelte';
import AirplaneTiltDuotone from 'phosphor-icons-svelte/IconAirplaneTiltDuotone.svelte';
import ArrowCounterClockwiseDuotone from 'phosphor-icons-svelte/IconArrowCounterClockwiseDuotone.svelte';
import ArrowsLeftRightDuotone from 'phosphor-icons-svelte/IconArrowsLeftRightDuotone.svelte';
import BabyDuotone from 'phosphor-icons-svelte/IconBabyDuotone.svelte';
import BarbellDuotone from 'phosphor-icons-svelte/IconBarbellDuotone.svelte';
import BedDuotone from 'phosphor-icons-svelte/IconBedDuotone.svelte';
import BicycleDuotone from 'phosphor-icons-svelte/IconBicycleDuotone.svelte';
import BriefcaseDuotone from 'phosphor-icons-svelte/IconBriefcaseDuotone.svelte';
import BuildingsDuotone from 'phosphor-icons-svelte/IconBuildingsDuotone.svelte';
import BusDuotone from 'phosphor-icons-svelte/IconBusDuotone.svelte';
import CarDuotone from 'phosphor-icons-svelte/IconCarDuotone.svelte';
import CoffeeDuotone from 'phosphor-icons-svelte/IconCoffeeDuotone.svelte';
import CoinsDuotone from 'phosphor-icons-svelte/IconCoinsDuotone.svelte';
import CreditCardDuotone from 'phosphor-icons-svelte/IconCreditCardDuotone.svelte';
import DropDuotone from 'phosphor-icons-svelte/IconDropDuotone.svelte';
import FilmSlateDuotone from 'phosphor-icons-svelte/IconFilmSlateDuotone.svelte';
import FirstAidDuotone from 'phosphor-icons-svelte/IconFirstAidDuotone.svelte';
import ForkKnifeDuotone from 'phosphor-icons-svelte/IconForkKnifeDuotone.svelte';
import GameControllerDuotone from 'phosphor-icons-svelte/IconGameControllerDuotone.svelte';
import GiftDuotone from 'phosphor-icons-svelte/IconGiftDuotone.svelte';
import GraduationCapDuotone from 'phosphor-icons-svelte/IconGraduationCapDuotone.svelte';
import HouseDuotone from 'phosphor-icons-svelte/IconHouseDuotone.svelte';
import LightningDuotone from 'phosphor-icons-svelte/IconLightningDuotone.svelte';
import MusicNotesDuotone from 'phosphor-icons-svelte/IconMusicNotesDuotone.svelte';
import PackageDuotone from 'phosphor-icons-svelte/IconPackageDuotone.svelte';
import PawPrintDuotone from 'phosphor-icons-svelte/IconPawPrintDuotone.svelte';
import PiggyBankDuotone from 'phosphor-icons-svelte/IconPiggyBankDuotone.svelte';
import ReceiptDuotone from 'phosphor-icons-svelte/IconReceiptDuotone.svelte';
import ScissorsDuotone from 'phosphor-icons-svelte/IconScissorsDuotone.svelte';
import ShieldCheckDuotone from 'phosphor-icons-svelte/IconShieldCheckDuotone.svelte';
import ShoppingBagDuotone from 'phosphor-icons-svelte/IconShoppingBagDuotone.svelte';
import ShoppingCartDuotone from 'phosphor-icons-svelte/IconShoppingCartDuotone.svelte';
import TShirtDuotone from 'phosphor-icons-svelte/IconTShirtDuotone.svelte';
import TagDuotone from 'phosphor-icons-svelte/IconTagDuotone.svelte';
import TrainDuotone from 'phosphor-icons-svelte/IconTrainDuotone.svelte';
import TrendUpDuotone from 'phosphor-icons-svelte/IconTrendUpDuotone.svelte';
import WifiHighDuotone from 'phosphor-icons-svelte/IconWifiHighDuotone.svelte';
import WrenchDuotone from 'phosphor-icons-svelte/IconWrenchDuotone.svelte';

type IconOption = {
  id: string;
  label: string;
  keywords: string[];
  Icon: Component<{ class?: string }>;
};

export const categoryIconGroups: { label: string; icons: IconOption[] }[] = [
  { label: 'Everyday spending', icons: [
    { id: 'fork-knife', label: 'Food & dining', keywords: ['food', 'dining', 'restaurant', 'meal', 'makan'], Icon: ForkKnifeDuotone },
    { id: 'coffee', label: 'Coffee & snacks', keywords: ['coffee', 'snacks', 'cafe', 'kopi'], Icon: CoffeeDuotone },
    { id: 'shopping-cart', label: 'Groceries', keywords: ['groceries', 'grocery', 'supermarket', 'belanja bulanan'], Icon: ShoppingCartDuotone },
    { id: 'shopping-bag', label: 'Shopping', keywords: ['shopping', 'retail', 'marketplace'], Icon: ShoppingBagDuotone },
    { id: 't-shirt', label: 'Clothing', keywords: ['clothing', 'clothes', 'apparel', 'baju'], Icon: TShirtDuotone },
    { id: 'scissors', label: 'Personal care', keywords: ['personal care', 'haircut', 'barber', 'salon'], Icon: ScissorsDuotone },
  ] },
  { label: 'Home & bills', icons: [
    { id: 'house', label: 'Rent & housing', keywords: ['rent', 'housing', 'mortgage', 'sewa'], Icon: HouseDuotone },
    { id: 'lightning', label: 'Electricity & utilities', keywords: ['electricity', 'utilities', 'power', 'listrik'], Icon: LightningDuotone },
    { id: 'drop', label: 'Water', keywords: ['water', 'air pdam', 'pdam'], Icon: DropDuotone },
    { id: 'wifi-high', label: 'Internet & phone', keywords: ['internet', 'wifi', 'phone', 'mobile', 'pulsa'], Icon: WifiHighDuotone },
    { id: 'receipt', label: 'Bills & fees', keywords: ['bill', 'bills', 'fee', 'fees', 'tagihan'], Icon: ReceiptDuotone },
    { id: 'wrench', label: 'Maintenance & repairs', keywords: ['maintenance', 'repairs', 'home improvement'], Icon: WrenchDuotone },
  ] },
  { label: 'Transport & travel', icons: [
    { id: 'car', label: 'Car & fuel', keywords: ['car', 'fuel', 'gasoline', 'parking', 'bensin', 'mobil'], Icon: CarDuotone },
    { id: 'train', label: 'Public transit', keywords: ['public transit', 'train', 'rail', 'commute', 'kereta'], Icon: TrainDuotone },
    { id: 'bus', label: 'Bus & rides', keywords: ['bus', 'rideshare', 'taxi', 'ride'], Icon: BusDuotone },
    { id: 'airplane-tilt', label: 'Flights & travel', keywords: ['travel', 'flights', 'airfare', 'holiday'], Icon: AirplaneTiltDuotone },
    { id: 'bed', label: 'Hotels & lodging', keywords: ['hotel', 'lodging', 'accommodation'], Icon: BedDuotone },
    { id: 'bicycle', label: 'Cycling', keywords: ['cycling', 'bicycle', 'bike', 'sepeda'], Icon: BicycleDuotone },
  ] },
  { label: 'Health & lifestyle', icons: [
    { id: 'first-aid', label: 'Health & pharmacy', keywords: ['health', 'medical', 'pharmacy', 'doctor', 'obat'], Icon: FirstAidDuotone },
    { id: 'barbell', label: 'Fitness & sports', keywords: ['fitness', 'gym', 'sports', 'olahraga'], Icon: BarbellDuotone },
    { id: 'film-slate', label: 'Entertainment', keywords: ['entertainment', 'cinema', 'movies', 'film'], Icon: FilmSlateDuotone },
    { id: 'game-controller', label: 'Games & hobbies', keywords: ['games', 'gaming', 'hobbies'], Icon: GameControllerDuotone },
    { id: 'graduation-cap', label: 'Education', keywords: ['education', 'tuition', 'school', 'course', 'sekolah'], Icon: GraduationCapDuotone },
    { id: 'gift', label: 'Gifts & donations', keywords: ['gift', 'donation', 'charity', 'hadiah', 'donasi'], Icon: GiftDuotone },
    { id: 'paw-print', label: 'Pets', keywords: ['pets', 'pet', 'vet'], Icon: PawPrintDuotone },
    { id: 'baby', label: 'Family & kids', keywords: ['family', 'kids', 'childcare', 'daycare', 'anak'], Icon: BabyDuotone },
    { id: 'music-notes', label: 'Music & events', keywords: ['music', 'concert', 'events'], Icon: MusicNotesDuotone },
  ] },
  { label: 'Income & money', icons: [
    { id: 'briefcase', label: 'Salary & work', keywords: ['salary', 'payroll', 'wages', 'work', 'gaji'], Icon: BriefcaseDuotone },
    { id: 'trend-up', label: 'Investments', keywords: ['investments', 'investment', 'dividends', 'stocks', 'investasi'], Icon: TrendUpDuotone },
    { id: 'coins', label: 'Side income', keywords: ['side income', 'freelance', 'bonus', 'income'], Icon: CoinsDuotone },
    { id: 'piggy-bank', label: 'Savings', keywords: ['savings', 'saving', 'tabungan'], Icon: PiggyBankDuotone },
    { id: 'credit-card', label: 'Credit & debt', keywords: ['credit card', 'credit', 'debt', 'loan payment'], Icon: CreditCardDuotone },
    { id: 'arrows-left-right', label: 'Transfer', keywords: ['transfer', 'internal movement'], Icon: ArrowsLeftRightDuotone },
    { id: 'arrow-counter-clockwise', label: 'Refunds', keywords: ['refund', 'refunds', 'reimbursement'], Icon: ArrowCounterClockwiseDuotone },
  ] },
  { label: 'General', icons: [
    { id: 'tag', label: 'General & other', keywords: ['general', 'other', 'miscellaneous'], Icon: TagDuotone },
    { id: 'shield-check', label: 'Insurance', keywords: ['insurance', 'protection', 'asuransi'], Icon: ShieldCheckDuotone },
    { id: 'buildings', label: 'Taxes & government', keywords: ['tax', 'taxes', 'government', 'pajak'], Icon: BuildingsDuotone },
    { id: 'package', label: 'Shipping & parcels', keywords: ['shipping', 'delivery', 'parcel', 'ongkir'], Icon: PackageDuotone },
  ] },
];

const icons = categoryIconGroups.flatMap(group => group.icons);
const byId = new Map(icons.map(icon => [icon.id, icon]));
const normalize = (value: string) => value.normalize('NFKD').toLowerCase().replace(/\p{Mark}/gu, '')
  .replace(/[^\p{Letter}\p{Number}]+/gu, ' ').trim();

export function resolveCategoryIcon(categoryName: string, categoryKind: string, explicitIcon?: string | null) {
  const stored = explicitIcon ? byId.get(explicitIcon) : undefined;
  if (stored) return stored;

  const name = ' ' + normalize(categoryName) + ' ';
  let best: IconOption | undefined;
  let specificity = 0;
  for (const icon of icons) {
    for (const keyword of icon.keywords) {
      if (keyword.length > specificity && name.includes(' ' + keyword + ' ')) {
        best = icon;
        specificity = keyword.length;
      }
    }
  }
  const fallback = categoryKind === 'income' ? 'coins' : categoryKind === 'transfer' ? 'arrows-left-right' : 'tag';
  return best ?? byId.get(fallback)!;
}
