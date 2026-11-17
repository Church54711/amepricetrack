package app.amepricetrack

import android.app.Application
import app.amepricetrack.data.ApiClient
import app.amepricetrack.data.Session
import app.amepricetrack.work.AlertWorker

class App : Application() {
    lateinit var session: Session
        private set
    lateinit var api: ApiClient
        private set

    override fun onCreate() {
        super.onCreate()
        session = Session(this)
        api = ApiClient(serverUrl = { session.state.value.serverUrl }, token = { session.state.value.token })
        AlertWorker.createChannel(this)
        if (session.state.value.signedIn) AlertWorker.schedule(this)
    }
}
