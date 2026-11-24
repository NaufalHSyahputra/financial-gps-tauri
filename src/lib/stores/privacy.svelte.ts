import { commands, type AppPreferences_Serialize, type Currency, type Money } from '../drivers/runtime';
import { formatMinor, toMinor } from '../money';
import { session } from './session.svelte';

class PrivacyStore {
  prefs = $state<AppPreferences_Serialize | null>(null);
  currencies = $state<Currency[]>([]);
  get masked() { return this.prefs?.privacyMask ?? false; }
  apply(prefs: AppPreferences_Serialize) { this.prefs = prefs; }
  async toggle() { this.apply(await session.call(commands.appPreferencesSet({ privacyMask: !this.masked }), false)); }
  scale(code: string) { return this.currencies.find(item => item.code === code)?.scale; }
  locale(code: string) { return this.prefs?.numberFormat === 'indonesian' || code === 'IDR' ? 'id-ID' : 'en-US'; }
  amount(code: string, value: string): Money {
    const scale = this.scale(code);
    if (scale === undefined) throw new Error('Select a supported currency.');
    return { currency: code, minor: toMinor(value, scale) };
  }
  money(money: Money | null | undefined) {
    if (!money) return '—';
    return this.masked ? '••••' : formatMinor(money, this.scale(money.currency), this.locale(money.currency));
  }
  date(value: string | null | undefined) {
    if (!value) return '—';
    const [y,m,d] = value.split('T')[0].split('-');
    return this.prefs?.dateFormat === 'dayFirst' ? `${d}/${m}/${y}` :
      this.prefs?.dateFormat === 'monthFirst' ? `${m}/${d}/${y}` : value.split('T')[0];
  }
  visible(key: string) {
    const aliases = key === 'groups' ? ['groups','shared','sharedgroups'] : key === 'fx' ? ['fx','fxprovider'] : [key];
    return !(this.prefs?.hiddenFeatures ?? []).some(item => aliases.includes(item.toLowerCase().replace(/[-_]/g, '')));
  }
}

export const privacy = new PrivacyStore();
