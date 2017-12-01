pipeline {
  agent any

  stages {
    stage('build') {
      steps {
        build 'janus-gateway-hab-package'
      }
    }
  }

  post {
     always {
       deleteDir()
     }
   }
}
