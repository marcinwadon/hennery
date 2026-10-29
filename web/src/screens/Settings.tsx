import { hats as listHats } from '../api/manage'
import { useClient } from '../app-client'
import AccountSettings, { AccountLogins } from '../components/AccountSettings'
import CollectorSettings from '../components/CollectorSettings'
import NotificationSettings from '../components/NotificationSettings'
import DelegationSettings from '../components/DelegationSettings'
import DelegationLimitsSettings from '../components/DelegationLimitsSettings'
import { DefaultsSettings } from '../components/HatDefaultsSummary'
import { useResource } from '../hooks/useResource'
import VersionInfo from '../components/VersionInfo'
import '../manage.css'
import { Link } from '../router'

export default function Settings({ push, sessions }: { push: boolean; sessions: boolean }) {
  const client = useClient()
  const hats = useResource(() => sessions ? listHats(client) : Promise.resolve([]), [client, sessions])
  return <div className="manage device-settings">
    <header className="manage-head"><h1>Settings</h1></header>
    <p className="manage-subtitle">Your account, browser and collector preferences.</p>
    <AccountSettings />
    {sessions && <p><Link to="/external-mcp">Manage external MCP client access</Link></p>}
    {push && <NotificationSettings push={push} sessions={sessions} />}
    <CollectorSettings push={push} sessions={sessions} hats={hats.data} />
    {hats.error && <p role="alert">Unable to load hats. <button className="btn btn-ghost" onClick={hats.reload}>Retry hats</button></p>}
    {sessions && <DelegationSettings hats={hats.data} />}
    {sessions && <DelegationLimitsSettings />}
    {sessions && <DefaultsSettings hats={hats.data} />}
    <AccountLogins />
    <section className="about-version" aria-label="About"><h2>About</h2><VersionInfo /></section>
  </div>
}
